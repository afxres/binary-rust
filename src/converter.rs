use crate::{
    allocator::Allocator,
    internal::{error_helper, length},
};

#[allow(unused)]
pub trait Converter<T>: crate::Converter {
    fn encode(&self, allocator: &mut Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>>;

    fn encode_auto(&self, allocator: &mut Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>> {
        if self.length() != 0 {
            return self.encode(allocator, item);
        } else {
            return self.encode_with_length_prefix(allocator, item);
        }
    }

    fn encode_with_length_prefix(&self, allocator: &mut Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>> {
        let anchor = allocator.anchor()?;
        self.encode(allocator, item)?;
        allocator.finish_anchor(anchor)?;
        Ok(())
    }

    fn decode(&self, span: &&[u8]) -> Result<T, Box<dyn std::error::Error>>;

    fn decode_auto(&self, span: &mut &[u8]) -> Result<T, Box<dyn std::error::Error>> {
        if self.length() != 0 {
            let (head, tail) = span.split_at_checked(self.length()).ok_or(error_helper::error_not_enough_bytes())?;
            *span = tail;
            return self.decode(&head);
        } else {
            return self.decode_with_length_prefix(span);
        }
    }

    fn decode_with_length_prefix(&self, span: &mut &[u8]) -> Result<T, Box<dyn std::error::Error>> {
        todo!()
    }
}

pub fn encode(allocator: &mut Allocator, number: usize) -> Result<(), Box<dyn std::error::Error>> {
    length::ensure_length_prefix_length(number)?;
    let prefix_length = length::encode_length_prefix_length(number);
    let source = allocator.assign(prefix_length)?;
    unsafe { length::encode_length_prefix(source, number, prefix_length) };
    Ok(())
}

pub fn encode_direct(span: &mut [u8], number: usize, bytes_written: &mut usize) -> Result<(), Box<dyn std::error::Error>> {
    length::ensure_length_prefix_length(number)?;
    let prefix_length = length::encode_length_prefix_length(number);
    if span.len() < prefix_length {
        return Err(error_helper::error_not_enough_bytes_to_write());
    }
    unsafe { length::encode_length_prefix(span.as_mut_ptr(), number, prefix_length) };
    *bytes_written = prefix_length;
    Ok(())
}
