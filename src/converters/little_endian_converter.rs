use crate::{
    converter::Converter,
    internal::{endian, error_helper},
};

pub struct LittleEndianConverter<T: 'static> {
    _t: std::marker::PhantomData<T>,
}

impl<T> LittleEndianConverter<T> {
    pub fn new() -> Self {
        Self { _t: std::marker::PhantomData }
    }
}

impl<T> crate::interface::Converter for LittleEndianConverter<T> {
    fn length(&self) -> usize {
        std::mem::size_of::<T>()
    }

    fn generic_argument(&self) -> std::any::TypeId {
        std::any::TypeId::of::<T>()
    }
}

impl<T> Converter<T> for LittleEndianConverter<T> {
    fn encode(&self, allocator: &mut crate::allocator::Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>> {
        unsafe { endian::encode_le_unaligned::<T>(allocator.assign(std::mem::size_of::<T>())?, item) };
        Ok(())
    }

    fn decode(&self, span: &&[u8]) -> Result<T, Box<dyn std::error::Error>> {
        if span.len() < std::mem::size_of::<T>() {
            return Err(error_helper::error_not_enough_bytes());
        }
        Ok(unsafe { endian::decode_le_unaligned::<T>(span.as_ptr()) })
    }
}
