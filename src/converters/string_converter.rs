use crate::converter::Converter;

pub struct StringConverter;

impl StringConverter {
    pub fn new() -> Self {
        Self {}
    }
}

impl crate::interface::Converter for StringConverter {
    fn length(&self) -> usize {
        0
    }

    fn generic_argument(&self) -> std::any::TypeId {
        std::any::TypeId::of::<std::string::String>()
    }
}

impl Converter<std::string::String> for StringConverter {
    fn encode(&self, allocator: &mut crate::allocator::Allocator, item: &std::string::String) -> Result<(), Box<dyn std::error::Error>> {
        if !item.is_empty() {
            unsafe { std::ptr::copy_nonoverlapping(item.as_ptr(), allocator.assign(item.len())?, item.len()) };
        }
        Ok(())
    }

    fn decode(&self, span: &&[u8]) -> Result<std::string::String, Box<dyn std::error::Error>> {
        Ok(std::str::from_utf8(&span)?.to_string())
    }
}
