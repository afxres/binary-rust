pub trait Converter {
    fn length(&self) -> usize;
    fn generic_argument(&self) -> std::any::TypeId;
}
