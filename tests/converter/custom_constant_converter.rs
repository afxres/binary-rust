use binary::{allocator::Allocator, converter, converter::Converter};

struct CustomConstantConverter<T: 'static> {
    _t: std::marker::PhantomData<T>,
}

impl<T> CustomConstantConverter<T> {
    fn new() -> Self {
        Self { _t: std::marker::PhantomData }
    }
}

impl<T> binary::interface::Converter for CustomConstantConverter<T> {
    fn length(&self) -> usize {
        std::mem::size_of::<T>()
    }

    fn generic_argument(&self) -> std::any::TypeId {
        std::any::TypeId::of::<T>()
    }
}

impl<T> Converter<T> for CustomConstantConverter<T> {
    fn encode(&self, allocator: &mut binary::allocator::Allocator, item: &T) -> Result<(), Box<dyn std::error::Error>> {
        allocator.append(unsafe { std::slice::from_raw_parts(std::ptr::from_ref(item).cast::<u8>(), std::mem::size_of::<T>()) })
    }

    fn decode(&self, span: &&[u8]) -> Result<T, Box<dyn std::error::Error>> {
        let part = span.split_at_checked(std::mem::size_of::<T>());
        let head = part.ok_or(Box::<dyn std::error::Error>::from("not enough bytes for custom constant type."))?.0;
        Ok(unsafe { head.as_ptr().cast::<T>().read_unaligned() })
    }
}

fn auto_methods_with_data<T: Eq + std::fmt::Debug + 'static>(source: T, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = CustomConstantConverter::<T>::new();
    converter.encode_auto(&mut allocator, &source)?;
    assert_eq!(allocator.length(), expected.len());
    assert_eq!(allocator[..], *expected);
    let mut span = &allocator[..];
    assert_eq!(span.len(), expected.len());
    let actual = converter.decode_auto(&mut span)?;
    assert_eq!(actual, source);
    assert_eq!(span.len(), 0);
    Ok(())
}

fn length_prefix_methods_with_data<T: Eq + std::fmt::Debug + 'static>(source: T, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = CustomConstantConverter::<T>::new();
    converter.encode_with_length_prefix(&mut allocator, &source)?;
    let mut bytes_expected = vec![0u8; 4];
    let mut bytes_written = 0usize;
    converter::encode_direct(&mut bytes_expected, std::mem::size_of::<T>(), &mut bytes_written)?;
    bytes_expected.resize(bytes_written, 0);
    bytes_expected.extend_from_slice(expected);
    assert_eq!(allocator.length(), bytes_expected.len());
    assert_eq!(allocator[..], bytes_expected[..]);
    let mut span = &allocator[..];
    let actual = converter.decode_with_length_prefix(&mut span)?;
    assert_eq!(actual, source);
    assert_eq!(span.len(), 0);
    Ok(())
}

#[test]
fn auto_methods() -> Result<(), Box<dyn std::error::Error>> {
    auto_methods_with_data(42i32, &42i32.to_ne_bytes())?;
    auto_methods_with_data(97i64, &97i64.to_ne_bytes())?;
    Ok(())
}

#[test]
fn length_prefix_methods() -> Result<(), Box<dyn std::error::Error>> {
    length_prefix_methods_with_data(127u32, &127u32.to_ne_bytes())?;
    length_prefix_methods_with_data(768u64, &768u64.to_ne_bytes())?;
    Ok(())
}

#[test]
fn decode_auto_not_enough_bytes() {
    let converter = CustomConstantConverter::<u16>::new();
    let buffer = [0u8; 1];
    let mut span = &buffer[..];
    let result = converter.decode_auto(&mut span);
    let binding = result.unwrap_err();
    let error = binding.downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "not enough bytes.")
}
