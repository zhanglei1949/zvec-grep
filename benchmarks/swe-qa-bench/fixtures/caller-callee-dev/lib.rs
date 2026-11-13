/// Normalize a value before storing it.
pub fn normalize(value: i32) -> i32 { value.max(0) }

pub fn save(value: i32) -> i32 { normalize(value) }

pub fn import_record(value: i32) -> i32 { save(value) }

pub fn export_record(value: i32) -> i32 { save(value) }

/// A leaf with no resolved callers in this workspace.
pub fn orphan_leaf(value: i32) -> i32 { value }

pub mod storage {
    pub fn save(value: i32) -> i32 { super::normalize(value) }

    pub fn flush(value: i32) -> i32 { save(value) }
}
