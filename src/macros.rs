
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

macro_rules! meta {
    ($($k:expr => $v:expr),* $(,)?) => {
        vec![$((String::from($k), String::from($v))),*]
    };
}

macro_rules! nbt_get {
    ($v:expr, $first:literal $(| $rest:literal)* => $conv:expr) => {
        get($v, $first)$(.or_else(|| get($v, $rest)))*.and_then($conv)
    };
}
