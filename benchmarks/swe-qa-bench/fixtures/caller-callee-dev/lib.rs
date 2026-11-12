/// Normalize a value before storing it.
pub fn normalize(value: i32) -> i32 { value.max(0) }

pub fn save(value: i32) -> i32 { normalize(value) }

pub fn import_record(value: i32) -> i32 { save(value) }
