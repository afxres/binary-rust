use binary::{allocator::Allocator, converter, converters::little_endian_converter::LittleEndianConverter};

fn base_methods_with_data<T: Eq + std::fmt::Debug + 'static>(source: T, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = Box::new(LittleEndianConverter::<T>::new()) as Box<dyn converter::Converter<T>>;
    converter.encode(&mut allocator, &source)?;
    assert_eq!(allocator.length(), expected.len());
    assert_eq!(allocator[..], *expected);
    let span = &allocator[..];
    assert_eq!(span.len(), expected.len());
    let actual = converter.decode(&span)?;
    assert_eq!(actual, source);
    Ok(())
}

fn auto_methods_with_data<T: Eq + std::fmt::Debug + 'static>(source: T, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = Box::new(LittleEndianConverter::<T>::new()) as Box<dyn converter::Converter<T>>;
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
    let converter = Box::new(LittleEndianConverter::<T>::new()) as Box<dyn converter::Converter<T>>;
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
fn base_methods() -> Result<(), Box<dyn std::error::Error>> {
    base_methods_with_data(0x7Fi8, &0x7Fi8.to_ne_bytes())?;
    base_methods_with_data(0x1234i16, &0x1234i16.to_ne_bytes())?;
    base_methods_with_data(0x12345678i32, &0x12345678i32.to_ne_bytes())?;
    base_methods_with_data(0x1234567890ABCDEFi64, &0x1234567890ABCDEFi64.to_ne_bytes())?;
    Ok(())
}

#[test]
fn auto_methods() -> Result<(), Box<dyn std::error::Error>> {
    auto_methods_with_data(0x7Fi8, &0x7Fi8.to_ne_bytes())?;
    auto_methods_with_data(0x1234i16, &0x1234i16.to_ne_bytes())?;
    auto_methods_with_data(0x12345678i32, &0x12345678i32.to_ne_bytes())?;
    auto_methods_with_data(0x1234567890ABCDEFi64, &0x1234567890ABCDEFi64.to_ne_bytes())?;
    Ok(())
}

#[test]
fn length_prefix_methods() -> Result<(), Box<dyn std::error::Error>> {
    length_prefix_methods_with_data(0x7Fi8, &0x7Fi8.to_ne_bytes())?;
    length_prefix_methods_with_data(0x1234i16, &0x1234i16.to_ne_bytes())?;
    length_prefix_methods_with_data(0x12345678i32, &0x12345678i32.to_ne_bytes())?;
    length_prefix_methods_with_data(0x1234567890ABCDEFi64, &0x1234567890ABCDEFi64.to_ne_bytes())?;
    Ok(())
}

#[test]
fn decode_not_enough_bytes() {
    let converter = Box::new(LittleEndianConverter::<u16>::new()) as Box<dyn converter::Converter<u16>>;
    let buffer = [0u8; 1];
    let span = &buffer[..];
    let result = converter.decode(&span);
    let binding = result.unwrap_err();
    let error = binding.downcast_ref::<std::io::Error>().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "not enough bytes.")
}
