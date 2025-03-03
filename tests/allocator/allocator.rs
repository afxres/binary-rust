use binary::{self, allocator::Allocator};
use std::error::Error;

#[test]
fn allocator_new() {
    let allocator = Allocator::new();
    assert_eq!(allocator.length(), 0);
    assert_eq!(allocator.capacity(), 0);
    assert_eq!(allocator.max_capacity(), i32::MAX as usize);
}

#[test]
fn allocator_new_display() {
    let allocator = Allocator::new();
    assert_eq!(allocator.to_string(), format!("length = 0, capacity = 0, max capacity = {max}", max = i32::MAX));
}

#[test]
fn allocator_new_slice() {
    let allocator = Allocator::new();
    let span = &allocator[..];
    assert_eq!(span.len(), 0);
}

#[test]
fn allocator_append() -> Result<(), Box<dyn Error>> {
    let sizes = vec![1, 128, 255];
    for size in sizes {
        let mut allocator = Allocator::new();
        let source = b"1".repeat(size);
        allocator.append(&source)?;
        let span = &allocator[..];
        assert_eq!(span.len(), size);
        assert_eq!(span, source);
        assert_eq!(allocator.length(), size);
        assert_eq!(allocator.capacity(), 256);
        assert_eq!(allocator.to_string(), format!("length = {size}, capacity = 256, max capacity = {max}", max = i32::MAX));
    }
    Ok(())
}
