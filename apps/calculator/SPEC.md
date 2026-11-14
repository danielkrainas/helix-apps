# Calculator

Drop-in replacement for GNOME Calculator.

- **App ID**: `org.helix.Calculator`
- **Binary**: `helix-calculator`
- **Crate**: `apps/calculator`

## Purpose
Basic, advanced, financial, and programming calculator.

## Main windows
- Single window with mode switcher.
- History.

## Contracts used
- GSettings: `org.gnome.calculator`.
- No special portals required.

## Non-goals
- Exact button layout clone.
- Arbitrary-precision arithmetic. Helix uses IEEE-754 doubles for real
  arithmetic and 64-bit integers for programming mode.
- Currency conversion (needs a network rate source; tracked as later work).

---

## Behaviour (derived spec)

### Window
- One `AdwApplicationWindow`. The header bar holds a **mode switcher**
  (Basic, Advanced, Financial, Programming) and a primary menu with
  *Preferences*, *Keyboard Shortcuts* and *About*.
- Below the header: the **history** (scrollable list of past
  `expression = result` rows, newest at the bottom), then the **entry line**,
  then the button panel for the current mode.
- Clicking a history row's expression inserts it into the entry; clicking its
  result inserts the result.
- Window size and the current mode persist across runs.
- Accessible names: a keypad button whose label is a symbol or short form
  is named by the operation it performs — its tooltip without the
  bracketed shortcut hint ("÷" → "Divide", "x²" → "Square", "⌫" →
  "Backspace", "C" → "Clear", "|x|" → "Absolute value", "ans" → "Last
  answer"); "(" / ")" are "Open parenthesis" / "Close parenthesis".
  Digits and hex letters keep their label as the name. Tooltips still show
  the shortcut. The mode, number base and word size drop-downs are named
  "Calculator mode", "Number base" and "Word size".

### Input
- The entry accepts free typing. Everything a button does can be typed.
- `Enter` / `=` / `KP_Enter` evaluates. `Escape` clears the entry. `Ctrl+Z`
  undoes the last evaluation (restores the previous entry).
- After evaluation the entry shows the result, selected so typing a digit
  starts a new expression while typing an operator continues from it.
- Errors (syntax, division by zero, domain) are shown inline below the entry
  and leave the expression untouched with the cursor at the error position.

### Expression language
| Feature | Syntax |
|---|---|
| Operators | `+ - × ÷ * / ^ mod`, unary `-`, postfix `!` (factorial), postfix `%` (divide by 100; `a + b%` means `a + a·b/100`) |
| Superscripts | `x²`, `x³`, `x⁻¹`, any `⁰-⁹` sequence as exponent |
| Roots | `√x`, `∛x`, `n√x` (n-th root) |
| Implicit multiplication | `2π`, `3(4+1)`, `(1+2)(3+4)`, `2sin 30` |
| Constants | `π`/`pi`, `e`, `rand` (uniform [0,1)), `ans` (last result) |
| Variables | `name = expr` assigns; names are letters, not shadowing functions |
| Abs | `abs x` or `|x|` |
| Functions | `sin cos tan asin acos atan sinh cosh tanh asinh acosh atanh ln log log2 exp sqrt cbrt int frac round floor ceil sgn` (also `sin⁻¹` etc.). Arguments may be parenthesised or a single primary (`sin 30`). `log` is base 10. |
| Angles | Trig functions use the *angle unit* preference (degrees, radians, gradians). |
| Unit conversion | `<value> <unit> in <unit>` (e.g. `12 in in cm`, `100 degF in degC`) for length, area, volume, mass, time, temperature, speed, data, angle |
| Precedence (low→high) | assignment, `in`, `+ -`, `× ÷ mod` & implicit ×, unary `-`, `^` (right assoc), postfix `! %` |

### Programming mode
- Integer arithmetic on signed 64-bit values masked to the *word size*
  (8, 16, 32, 64 bits).
- Number base selector: binary, octal, decimal, hexadecimal. Input digits
  outside the current base are disabled; letters `A–F` are digits in hex.
  Literals may carry an explicit base: `0x1F`, `0b101`, `0o17`, or subscript
  `1F₁₆`.
- Additional operators: `and or xor not`, `<<`, `>>`; functions `ones`
  (one's complement), `twos` (two's complement).
- The result is shown in the selected base, with a bit panel showing the
  value's bits (clickable to toggle) grouped in nibbles.

### Financial mode
Functions available as buttons, each opening a small form, and also typeable:
| Function | Meaning |
|---|---|
| `ctrm(rate; fv; pv)` | Compounding term: periods to grow pv to fv |
| `ddb(cost; life; period)` | Double-declining depreciation for a period |
| `fv(pmt; rate; n)` | Future value of an annuity |
| `gpm(cost; margin)` | Gross-profit-margin price |
| `pmt(principal; rate; n)` | Periodic payment |
| `pv(pmt; rate; n)` | Present value of an annuity |
| `rate(fv; pv; n)` | Periodic interest rate |
| `sln(cost; salvage; life)` | Straight-line depreciation |
| `syd(cost; salvage; life; period)` | Sum-of-years-digits depreciation |
| `term(pmt; fv; rate)` | Payment periods to reach future value |

Arguments are separated with `;` (`,` is accepted too where unambiguous).

### Result formatting
Controlled by preferences:
- **Number format**: automatic (fixed, switching to scientific for very large
  or small magnitudes), fixed, scientific, engineering.
- **Accuracy**: decimal places (0–15, default 9).
- **Show thousands separators** (default off) and **show trailing zeroes**
  (default off).
- In programming mode results use the selected base.

### GSettings (`org.gnome.calculator`)
Keys read and written (others ignored):
`accuracy` (i), `word-size` (i), `base` (i), `show-thousands` (b),
`show-zeroes` (b), `number-format` (enum `automatic|fixed|scientific|engineering`),
`angle-units` (enum `degrees|radians|gradians`), `button-mode`
(enum `basic|advanced|financial|programming|keyboard`; `keyboard` is treated as
`basic`), `window-maximized` (b), `window-size` (`(ii)`).
If the schema is not installed, defaults are used in-memory.

### History
- Kept for the session in memory and persisted to
  `$XDG_DATA_HOME/helix-calculator/history.json` (last 100 entries).
- *Clear history* in the primary menu.

### Keyboard shortcuts
`Ctrl+B/A/F/P` switch to Basic/Advanced/Financial/Programming mode,
`Ctrl+,` preferences, `Ctrl+Q` quit, `F1` about.

### Command line
`helix-calculator --solve EXPR` prints the result to stdout and exits
(exit status 1 with the error on stderr on failure). `-e EXPR` opens the
window with `EXPR` in the entry.
