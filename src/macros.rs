// Small function-like helpers to kill construction boilerplate.
// Kept as macro_rules! (no proc-macro crate) and brought into scope
// crate-wide via `#[macro_use] mod macros;` in main.rs.

// `copies!["Copy TP" => tp, "Copy Coords" => coords]`
// -> Vec<CopyAction> with each label/value coerced to String.
macro_rules! copies {
    ($($label:expr => $value:expr),* $(,)?) => {
        vec![$(
            $crate::model::CopyAction {
                label: String::from($label),
                value: String::from($value),
            }
        ),*]
    };
}

// `meta!["Type" => id, "Position" => pos]`
// -> Vec<(String, String)>.
macro_rules! meta {
    ($($k:expr => $v:expr),* $(,)?) => {
        vec![$((String::from($k), String::from($v))),*]
    };
}

// `nbt_get!(v, "count" | "Count" => as_i64)`
// -> the first present key run through `and_then(conv)`.
// Expands using the `get` fn in scope at the call site.
macro_rules! nbt_get {
    ($v:expr, $first:literal $(| $rest:literal)* => $conv:expr) => {
        get($v, $first)$(.or_else(|| get($v, $rest)))*.and_then($conv)
    };
}
