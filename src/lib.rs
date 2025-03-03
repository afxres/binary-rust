pub(crate) mod internal;

pub mod allocator;
pub mod converter;

pub trait Converter {
    fn length(&self) -> usize;
    fn generic_argument(&self) -> std::any::TypeId;
}
