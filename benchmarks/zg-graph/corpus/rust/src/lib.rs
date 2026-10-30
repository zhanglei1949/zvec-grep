pub mod calc;
use crate::calc::{add as plus, twice};

pub fn run() -> i32 {
    twice(plus(1, 2))
}

pub fn shadow(plus: fn(i32, i32) -> i32) -> i32 {
    plus(2, 3)
}
