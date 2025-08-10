use binary::{allocator::Allocator, converter, converters::string_converter::StringConverter, interface::Converter};

fn base_methods_with_data(source: std::string::String, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = Box::new(StringConverter::new()) as Box<dyn converter::Converter<std::string::String>>;
    converter.encode(&mut allocator, &source)?;
    assert_eq!(allocator.length(), expected.len());
    assert_eq!(allocator[..], *expected);
    let span = &allocator[..];
    assert_eq!(span.len(), expected.len());
    let actual = converter.decode(&span)?;
    assert_eq!(actual, source);
    Ok(())
}

fn auto_methods_with_data(source: std::string::String, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = Box::new(StringConverter::new()) as Box<dyn converter::Converter<std::string::String>>;
    converter.encode_auto(&mut allocator, &source)?;
    let mut bytes_expected = vec![0u8; 4];
    let mut bytes_written = 0usize;
    converter::encode_direct(&mut bytes_expected, source.len(), &mut bytes_written)?;
    bytes_expected.resize(bytes_written, 0);
    bytes_expected.extend_from_slice(expected);
    assert_eq!(allocator.length(), bytes_expected.len());
    assert_eq!(allocator[..], bytes_expected[..]);
    let mut span = &allocator[..];
    let actual = converter.decode_auto(&mut span)?;
    assert_eq!(actual, source);
    assert_eq!(span.len(), 0);
    Ok(())
}

fn length_prefix_methods_with_data(source: std::string::String, expected: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let mut allocator = Allocator::new();
    let converter = Box::new(StringConverter::new()) as Box<dyn converter::Converter<std::string::String>>;
    converter.encode_with_length_prefix(&mut allocator, &source)?;
    let mut bytes_expected = vec![0u8; 4];
    let mut bytes_written = 0usize;
    converter::encode_direct(&mut bytes_expected, source.len(), &mut bytes_written)?;
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
fn base_info() {
    let converter = StringConverter::new();
    assert_eq!(converter.length(), 0);
    assert_eq!(converter.generic_argument(), std::any::TypeId::of::<std::string::String>())
}

#[test]
fn base_methods() -> Result<(), Box<dyn std::error::Error>> {
    base_methods_with_data("".to_string(), "".as_bytes())?;
    base_methods_with_data("Hello, world!".to_string(), "Hello, world!".as_bytes())?;
    Ok(())
}

#[test]
fn auto_methods() -> Result<(), Box<dyn std::error::Error>> {
    auto_methods_with_data("".to_string(), "".as_bytes())?;
    auto_methods_with_data("Hello, world!".to_string(), "Hello, world!".as_bytes())?;
    Ok(())
}

#[test]
fn length_prefix_methods() -> Result<(), Box<dyn std::error::Error>> {
    length_prefix_methods_with_data("".to_string(), "".as_bytes())?;
    length_prefix_methods_with_data("Hello, world!".to_string(), "Hello, world!".as_bytes())?;
    Ok(())
}

#[test]
fn decode_invalid_bytes() {
    let converter = Box::new(StringConverter::new()) as Box<dyn converter::Converter<std::string::String>>;
    let buffer = [0x80u8; 1];
    let span = &buffer[..];
    let result = converter.decode(&span);
    let binding = result.unwrap_err();
    let error = binding.downcast_ref::<core::str::Utf8Error>().unwrap();
    assert_eq!(error.error_len(), Some(1))
}
