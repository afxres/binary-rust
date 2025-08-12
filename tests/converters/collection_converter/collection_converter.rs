use std::{
    collections::{BTreeSet, HashSet},
    fmt::Debug,
};

use binary::{
    Converter,
    allocator::Allocator,
    converter,
    converters::{collection_converter::CollectionConverter, little_endian_converter::LittleEndianConverter, string_converter::StringConverter},
};

fn base_info_with_data<T: IntoIterator + 'static>(item_converter: Box<dyn converter::Converter<<T as IntoIterator>::Item>>)
where
    T: IntoIterator + FromIterator<<T as IntoIterator>::Item>,
{
    let converter = CollectionConverter::<T>::new(item_converter);
    assert_eq!(converter.length(), 0);
    assert_eq!(converter.generic_argument(), std::any::TypeId::of::<T>());
}

fn base_methods_with_data<T: IntoIterator + Debug + PartialEq + 'static>(item_converter: Box<dyn converter::Converter<<T as IntoIterator>::Item>>, item: &T) -> Result<(), Box<dyn std::error::Error>>
where
    T: IntoIterator + FromIterator<<T as IntoIterator>::Item>,
    for<'a> &'a T: IntoIterator<Item = &'a <T as IntoIterator>::Item>,
{
    let mut allocator = Allocator::new();
    let converter = Box::new(CollectionConverter::<T>::new(item_converter)) as Box<dyn converter::Converter<T>>;
    converter.encode(&mut allocator, &item)?;
    let span = &allocator[..];
    let actual = converter.decode(&span)?;
    assert_eq!(&actual, item);
    Ok(())
}

#[test]
fn base_info() {
    base_info_with_data::<Vec<i32>>(Box::new(LittleEndianConverter::<i32>::new()));
    base_info_with_data::<Vec<String>>(Box::new(StringConverter::new()));
    base_info_with_data::<HashSet<i32>>(Box::new(LittleEndianConverter::<i32>::new()));
    base_info_with_data::<BTreeSet<String>>(Box::new(StringConverter::new()));
}

#[test]
fn base_methods() -> Result<(), Box<dyn std::error::Error>> {
    base_methods_with_data::<Vec<i32>>(Box::new(LittleEndianConverter::<i32>::new()), &vec![1, 2, 3])?;
    base_methods_with_data::<Vec<String>>(Box::new(StringConverter::new()), &vec!["Alpha".to_string(), "Bravo".to_string()])?;
    base_methods_with_data::<HashSet<i32>>(Box::new(LittleEndianConverter::<i32>::new()), &HashSet::from([1, 2, 3]))?;
    base_methods_with_data::<BTreeSet<String>>(Box::new(StringConverter::new()), &BTreeSet::from(["Alpha".to_string(), "Bravo".to_string()]))?;
    Ok(())
}
