pub(crate) mod internal;

pub mod converters;

pub mod allocator;
pub mod converter;
pub mod generator;

pub trait Converter {
    fn length(&self) -> usize;
    fn generic_argument(&self) -> std::any::TypeId;
}

pub trait Generator {
    fn add_converter(&mut self, converter: &std::sync::Arc<dyn Converter>);
    fn get_converter(&self, r#type: std::any::TypeId) -> Option<std::sync::Arc<dyn Converter>>;
}
