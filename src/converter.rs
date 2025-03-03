use crate::{allocator::Allocator, internal::error_helper};

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
        todo!()
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
