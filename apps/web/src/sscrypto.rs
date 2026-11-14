//! Cryptography for the Secret Service `dh-ietf1024-sha256-aes128-cbc-pkcs7`
//! transfer algorithm (freedesktop Secret Service API 0.2, "Transfer of
//! Secrets"):
//!
//! - Diffie-Hellman over the RFC 2409 Second Oakley Group (1024-bit MODP
//!   prime, generator 2). Public keys travel as unsigned big-endian byte
//!   arrays (`ay`).
//! - The shared secret, as a 128-byte big-endian number, is the input keying
//!   material of HKDF-SHA256 with no salt and empty info; the first 16 bytes
//!   of output are the AES-128 key.
//! - Each secret is AES-128-CBC encrypted with PKCS#7 padding; the random
//!   16-byte IV goes in the secret's `parameters`.
//!
//! Modular exponentiation is constant-time (`crypto-bigint`); private keys,
//! shared secrets, derived keys and plaintext buffers are zeroized on drop.

use aes::Aes128;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use crypto_bigint::modular::runtime_mod::{DynResidue, DynResidueParams};
use crypto_bigint::{Encoding, U1024};
use rand::RngCore;
use zeroize::{Zeroize, Zeroizing};

pub const ALGORITHM: &str = "dh-ietf1024-sha256-aes128-cbc-pkcs7";

/// RFC 2409 section 6.2, "Second Oakley Group" prime.
pub const PRIME: U1024 = U1024::from_be_hex(concat!(
    "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD1",
    "29024E088A67CC74020BBEA63B139B22514A08798E3404DD",
    "EF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245",
    "E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7ED",
    "EE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381",
    "FFFFFFFFFFFFFFFF",
));
pub const GENERATOR: U1024 = U1024::from_u8(2);
const BYTES: usize = 128;

pub type Key = Zeroizing<[u8; 16]>;

fn modpow(base: &U1024, exp: &U1024) -> U1024 {
    let params = DynResidueParams::new(&PRIME);
    DynResidue::new(base, params).pow(exp).retrieve()
}

/// Minimal unsigned big-endian encoding (no leading zero bytes).
fn encode(n: &U1024) -> Vec<u8> {
    let bytes = n.to_be_bytes();
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(BYTES - 1);
    bytes[start..].to_vec()
}

/// Parses an unsigned big-endian number of at most 1024 bits (leading zero
/// bytes allowed).
fn decode(bytes: &[u8]) -> Option<U1024> {
    let start = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len());
    let bytes = &bytes[start..];
    if bytes.len() > BYTES {
        return None;
    }
    let mut buf = Zeroizing::new([0u8; BYTES]);
    buf[BYTES - bytes.len()..].copy_from_slice(bytes);
    Some(U1024::from_be_slice(&buf[..]))
}

/// One side of the key exchange.
pub struct KeyPair {
    private: Zeroizing<U1024>,
    public: Vec<u8>,
}

impl KeyPair {
    /// A fresh key pair with a random 1024-bit private exponent.
    pub fn generate() -> KeyPair {
        let mut bytes = Zeroizing::new([0u8; BYTES]);
        rand::rngs::OsRng.fill_bytes(&mut bytes[..]);
        KeyPair::from_private(&bytes)
    }

    /// A key pair from a known private exponent (tests).
    pub fn from_private(bytes: &[u8; BYTES]) -> KeyPair {
        let private = Zeroizing::new(U1024::from_be_slice(bytes));
        let public = encode(&modpow(&GENERATOR, &private));
        KeyPair { private, public }
    }

    /// The public key to send to the peer.
    pub fn public(&self) -> &[u8] {
        &self.public
    }

    /// Derives the AES key from the peer's public key; `None` when the peer
    /// key is malformed or outside 2..=p-2 (which would make the shared
    /// secret trivial).
    pub fn derive(&self, peer_public: &[u8]) -> Option<Key> {
        let peer = decode(peer_public)?;
        let p_minus_1 = PRIME.wrapping_sub(&U1024::ONE);
        if peer <= U1024::ONE || peer >= p_minus_1 {
            return None;
        }
        let mut shared = modpow(&peer, &self.private);
        let ikm = Zeroizing::new(shared.to_be_bytes());
        shared.zeroize();
        hkdf_key(&ikm[..])
    }
}

/// HKDF-SHA256 (no salt, empty info) truncated to an AES-128 key.
pub fn hkdf_key(ikm: &[u8]) -> Option<Key> {
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, ikm);
    let mut key = Zeroizing::new([0u8; 16]);
    hk.expand(&[], &mut key[..]).ok()?;
    Some(key)
}

/// Encrypts with a fresh random IV; returns (IV, ciphertext).
pub fn encrypt(key: &Key, plaintext: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut iv = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut iv);
    let ct = encrypt_with_iv(key, &iv, plaintext);
    (iv.to_vec(), ct)
}

pub fn encrypt_with_iv(key: &Key, iv: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
    cbc::Encryptor::<Aes128>::new(key[..].into(), iv.into())
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext)
}

/// Decrypts; `None` for a bad IV length or bad padding.
pub fn decrypt(key: &Key, iv: &[u8], ciphertext: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if iv.len() != 16 || ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
        return None;
    }
    cbc::Decryptor::<Aes128>::new(key[..].into(), iv.into())
        .decrypt_padded_vec_mut::<Pkcs7>(ciphertext)
        .ok()
        .map(Zeroizing::new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn prime_is_the_rfc_2409_group() {
        assert_eq!(PRIME.bits(), 1024);
        let bytes = PRIME.to_be_bytes();
        assert_eq!(&bytes[..12], &hex("FFFFFFFFFFFFFFFFC90FDAA2")[..]);
        assert_eq!(&bytes[116..], &hex("ECE65381FFFFFFFFFFFFFFFF")[..]);
    }

    #[test]
    fn hkdf_matches_rfc_5869_case_3() {
        // RFC 5869 A.3: SHA-256, IKM = 22 × 0x0b, empty salt and info.
        let key = hkdf_key(&[0x0b; 22]).unwrap();
        assert_eq!(key[..], hex("8da4e775a563c18f715f802a063c5a31")[..]);
    }

    #[test]
    fn aes_cbc_matches_sp800_38a() {
        // NIST SP 800-38A F.2.1 (CBC-AES128), first block; PKCS#7 then adds
        // one full padding block.
        let key =
            Zeroizing::new(<[u8; 16]>::try_from(hex("2b7e151628aed2a6abf7158809cf4f3c")).unwrap());
        let iv = <[u8; 16]>::try_from(hex("000102030405060708090a0b0c0d0e0f")).unwrap();
        let pt = hex("6bc1bee22e409f96e93d7e117393172a");
        let ct = encrypt_with_iv(&key, &iv, &pt);
        assert_eq!(ct.len(), 32);
        assert_eq!(ct[..16], hex("7649abac8119b246cee98e9b12e9197d")[..]);
        assert_eq!(&decrypt(&key, &iv, &ct).unwrap()[..], &pt[..]);
    }

    #[test]
    fn padding_and_round_trip() {
        let key = Zeroizing::new([7u8; 16]);
        for len in [0usize, 1, 15, 16, 17, 100] {
            let pt = vec![b'x'; len];
            let (iv, ct) = encrypt(&key, &pt);
            assert_eq!(iv.len(), 16);
            assert_eq!(ct.len(), (len / 16 + 1) * 16);
            assert_eq!(&decrypt(&key, &iv, &ct).unwrap()[..], &pt[..]);
        }
        let (iv, mut ct) = encrypt(&key, b"secret");
        assert!(decrypt(&Zeroizing::new([8u8; 16]), &iv, &ct).is_none_or(|p| &p[..] != b"secret"));
        assert!(decrypt(&key, &iv[..8], &ct).is_none());
        ct.pop();
        assert!(decrypt(&key, &iv, &ct).is_none());
    }

    #[test]
    fn small_exponents_give_known_public_keys() {
        let mut one = [0u8; BYTES];
        one[BYTES - 1] = 1;
        assert_eq!(KeyPair::from_private(&one).public(), &[2]);
        let mut ten = [0u8; BYTES];
        ten[BYTES - 1] = 10;
        assert_eq!(KeyPair::from_private(&ten).public(), &[0x04, 0x00]);
        // 2^1023 mod p with p = 2^1024 - k: still below p, so unchanged.
        let mut big = [0u8; BYTES];
        big[BYTES - 2] = 0x03;
        big[BYTES - 1] = 0xff;
        let public = KeyPair::from_private(&big).public().to_vec();
        assert_eq!(public.len(), BYTES);
        assert_eq!(public[0], 0x80);
        assert!(public[1..].iter().all(|&b| b == 0));
    }

    #[test]
    fn both_sides_derive_the_same_key() {
        let a = KeyPair::generate();
        let b = KeyPair::generate();
        let ka = a.derive(b.public()).unwrap();
        let kb = b.derive(a.public()).unwrap();
        assert_eq!(ka[..], kb[..]);
        let c = KeyPair::generate();
        assert_ne!(a.derive(c.public()).unwrap()[..], ka[..]);
        // Leading zero bytes in the peer key are accepted.
        let mut padded = vec![0, 0];
        padded.extend_from_slice(b.public());
        assert_eq!(a.derive(&padded).unwrap()[..], ka[..]);
    }

    #[test]
    fn shared_secret_is_padded_to_128_bytes() {
        // With private key 1 and peer key 4 the shared secret is 4: the IKM
        // must be the full 128-byte big-endian encoding, not a single byte.
        let mut one = [0u8; BYTES];
        one[BYTES - 1] = 1;
        let k = KeyPair::from_private(&one).derive(&[4]).unwrap();
        let mut ikm = [0u8; BYTES];
        ikm[BYTES - 1] = 4;
        assert_eq!(k[..], hkdf_key(&ikm).unwrap()[..]);
        assert_ne!(k[..], hkdf_key(&[4]).unwrap()[..]);
    }

    #[test]
    fn rejects_degenerate_peer_keys() {
        let a = KeyPair::generate();
        let p = PRIME.to_be_bytes();
        let p1 = PRIME.wrapping_sub(&U1024::ONE).to_be_bytes();
        for bad in [
            &[][..],
            &[0][..],
            &[1][..],
            &p1[..],
            &p[..],
            &[1u8; 129][..],
        ] {
            assert!(a.derive(bad).is_none(), "{bad:?}");
        }
        assert!(a.derive(&[2]).is_some());
    }
}
