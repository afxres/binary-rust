//! Containers share `Vec<T>`'s raw payload: no element count, each element
//! through `auto`. See `binary::collections` module docs and `DESIGN.md` §17.3
//! for the three-way wire argument and the ordering caveat.

use binary::{from_slice, to_vec, write_named_member, Encode, Encoder, Error, Layout};
use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet, BinaryHeap, HashMap, HashSet, LinkedList, VecDeque};
use std::hash::BuildHasherDefault;

fn raw<T: Encode + ?Sized>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    v.encode(&mut e).unwrap();
    e.finish().unwrap()
}

fn named_bytes<T: Encode + ?Sized>(v: &T) -> Vec<u8> {
    let mut e = Encoder::new();
    write_named_member(&mut e, b"x", v).unwrap();
    e.finish().unwrap()
}

#[test]
fn test_container_layout_constants() {
    const {
        assert!(<VecDeque<i32> as Layout>::LEN == 0);
        assert!(<LinkedList<i32> as Layout>::LEN == 0);
        assert!(<BTreeSet<i32> as Layout>::LEN == 0);
        assert!(<BinaryHeap<i32> as Layout>::LEN == 0);
        assert!(<BTreeMap<i32, i32> as Layout>::LEN == 0);
        assert!(<HashMap<i32, i32> as Layout>::LEN == 0);
        assert!(<HashSet<i32> as Layout>::LEN == 0);
        assert!(<Cow<'static, str> as Layout>::LEN == 0);
    }
}

#[test]
fn test_vec_deque_matches_vec_including_wrapped_slices() {
    let plain = VecDeque::from(vec![1i32, -2, 3]);
    assert_eq!(to_vec(&plain).unwrap(), to_vec(&vec![1i32, -2, 3]).unwrap());
    assert_eq!(
        from_slice::<VecDeque<i32>>(&to_vec(&plain).unwrap()).unwrap(),
        plain
    );

    // `push_front` past the initial capacity wraps the ring: `as_slices()`
    // becomes two runs, and both still have to produce the Vec byte order.
    let mut wrapped = VecDeque::from(vec![2i32, 3, 4]);
    wrapped.push_front(1);
    let (head, tail) = wrapped.as_slices();
    assert!(!tail.is_empty(), "probe should exercise the two-run path");
    assert!(!head.is_empty());
    let expected: Vec<i32> = wrapped.iter().copied().collect();
    assert_eq!(to_vec(&wrapped).unwrap(), to_vec(&expected).unwrap());
    assert_eq!(raw(&wrapped), raw(&expected));
    assert_eq!(
        from_slice::<VecDeque<i32>>(&to_vec(&wrapped).unwrap()).unwrap(),
        wrapped
    );
}

#[test]
fn test_linked_list_and_ordered_sets_match_vec() {
    let list: LinkedList<i32> = vec![1, -2, 3].into_iter().collect();
    assert_eq!(to_vec(&list).unwrap(), to_vec(&vec![1i32, -2, 3]).unwrap());
    assert_eq!(
        from_slice::<LinkedList<i32>>(&to_vec(&list).unwrap()).unwrap(),
        list
    );

    // BTreeSet is key-ordered, so its bytes are predictable and can be aligned
    // with an ordered container on the C# / C++ side.
    let set: BTreeSet<i32> = [3, 1, 2].into_iter().collect();
    assert_eq!(to_vec(&set).unwrap(), to_vec(&vec![1i32, 2, 3]).unwrap());
    assert_eq!(
        from_slice::<BTreeSet<i32>>(&to_vec(&set).unwrap()).unwrap(),
        set
    );

    // BinaryHeap iterates its internal array, not sorted order. Compare against
    // that same order, and compare contents after a round trip.
    let heap = BinaryHeap::from(vec![3i32, 1, 2]);
    let in_iter_order: Vec<i32> = heap.iter().copied().collect();
    assert_eq!(to_vec(&heap).unwrap(), to_vec(&in_iter_order).unwrap());
    let decoded: BinaryHeap<i32> = from_slice(&to_vec(&heap).unwrap()).unwrap();
    assert_eq!(decoded.into_sorted_vec(), heap.clone().into_sorted_vec());
}

#[test]
fn test_maps_match_vec_of_pairs() {
    // Fixed-size values: both sides take the O(1) `write_prefixed_bounded` path with
    // `(i32, i32)::LEN == 8`.
    let map = BTreeMap::from([(1i32, 10i32), (2, 20), (3, 30)]);
    let pairs = vec![(1i32, 10i32), (2, 20), (3, 30)];
    assert_eq!(to_vec(&map).unwrap(), to_vec(&pairs).unwrap());
    assert_eq!(raw(&map), raw(&pairs));
    assert_eq!(named_bytes(&map), named_bytes(&pairs));
    assert_eq!(
        from_slice::<BTreeMap<i32, i32>>(&to_vec(&map).unwrap()).unwrap(),
        map
    );

    // Variable-size values: both sides take the `write_prefixed` path, and the pair
    // itself contributes no header (K auto, V auto).
    let var_map = BTreeMap::from([(1i32, "a".to_string()), (2, "longer".to_string())]);
    let var_pairs: Vec<(i32, String)> = var_map.iter().map(|(k, v)| (*k, v.clone())).collect();
    assert_eq!(to_vec(&var_map).unwrap(), to_vec(&var_pairs).unwrap());
    assert_eq!(named_bytes(&var_map), named_bytes(&var_pairs));
    assert_eq!(
        from_slice::<BTreeMap<i32, String>>(&to_vec(&var_map).unwrap()).unwrap(),
        var_map
    );
}

#[test]
fn test_hash_containers_match_vec_in_iteration_order() {
    // HashMap / HashSet carry a per-process random seed, so only their own
    // iteration order is a valid byte-level expectation. Cross-language byte
    // equality is explicitly NOT promised (DESIGN §17.3).
    let map = HashMap::from([(1i32, "a".to_string()), (2, "bb".to_string())]);
    let pairs: Vec<(i32, String)> = map.iter().map(|(k, v)| (*k, v.clone())).collect();
    assert_eq!(to_vec(&map).unwrap(), to_vec(&pairs).unwrap());
    assert_eq!(named_bytes(&map), named_bytes(&pairs));
    assert_eq!(
        from_slice::<HashMap<i32, String>>(&to_vec(&map).unwrap()).unwrap(),
        map
    );

    let set: HashSet<i32> = [1, 2, 3].into_iter().collect();
    let in_iter_order: Vec<i32> = set.iter().copied().collect();
    assert_eq!(to_vec(&set).unwrap(), to_vec(&in_iter_order).unwrap());
    assert_eq!(
        from_slice::<HashSet<i32>>(&to_vec(&set).unwrap()).unwrap(),
        set
    );
}

#[test]
fn test_hasher_is_generic_no_hashbrown_needed() {
    type Fixed = BuildHasherDefault<DefaultHasher>;
    let mut map: HashMap<i32, i32, Fixed> = HashMap::default();
    map.insert(7, 70);
    let pairs: Vec<(i32, i32)> = map.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(to_vec(&map).unwrap(), to_vec(&pairs).unwrap());
    assert_eq!(
        from_slice::<HashMap<i32, i32, Fixed>>(&to_vec(&map).unwrap()).unwrap(),
        map
    );

    let mut set: HashSet<i32, Fixed> = HashSet::default();
    set.insert(7);
    assert_eq!(
        from_slice::<HashSet<i32, Fixed>>(&to_vec(&set).unwrap()).unwrap(),
        set
    );
}

#[test]
fn test_cow_is_wire_transparent_and_decodes_owned() {
    // Encoding forwards all three methods to `B`, so borrowed and owned
    // produce exactly the bytes of the inner type (including str's O(1)
    // prefix override).
    let borrowed: Cow<'_, str> = Cow::Borrowed("hi");
    let owned: Cow<'_, str> = Cow::Owned("hi".to_string());
    assert_eq!(to_vec(&borrowed).unwrap(), to_vec(&"hi").unwrap());
    assert_eq!(to_vec(&owned).unwrap(), to_vec(&"hi".to_string()).unwrap());
    assert_eq!(raw(&borrowed), raw(&owned));
    assert_eq!(named_bytes(&borrowed), named_bytes(&owned));
    let expected_str: Cow<'static, str> = Cow::Owned("hi".to_string());
    assert_eq!(
        from_slice::<Cow<'static, str>>(&to_vec(&borrowed).unwrap()).unwrap(),
        expected_str
    );

    let bytes = [1u8, 2, 3];
    let cow_bytes: Cow<'_, [u8]> = Cow::Borrowed(&bytes);
    assert_eq!(to_vec(&cow_bytes).unwrap(), to_vec(&bytes[..]).unwrap());
    let expected_bytes: Cow<'static, [u8]> = Cow::Owned(vec![1u8, 2, 3]);
    assert_eq!(
        from_slice::<Cow<'static, [u8]>>(&to_vec(&cow_bytes).unwrap()).unwrap(),
        expected_bytes
    );
}

#[test]
fn test_empty_containers() {
    // Raw one-shot encoding of an empty sequence is zero bytes; no element count.
    let empty_vec = Vec::<i32>::new();
    let cases: Vec<Vec<u8>> = vec![
        to_vec(&VecDeque::<i32>::new()).unwrap(),
        to_vec(&LinkedList::<i32>::new()).unwrap(),
        to_vec(&BTreeSet::<i32>::new()).unwrap(),
        to_vec(&BinaryHeap::<i32>::new()).unwrap(),
        to_vec(&BTreeMap::<i32, i32>::new()).unwrap(),
        to_vec(&HashMap::<i32, i32>::new()).unwrap(),
        to_vec(&HashSet::<i32>::new()).unwrap(),
    ];
    let expected = to_vec(&empty_vec).unwrap();
    for case in cases {
        assert_eq!(case, expected);
    }
    assert_eq!(
        from_slice::<BTreeMap<i32, i32>>(&expected).unwrap().len(),
        0
    );
    assert_eq!(from_slice::<HashSet<i32>>(&expected).unwrap().len(), 0);
}

#[test]
fn test_containers_nested_in_vec() {
    // `to_vec` is raw, so the outer Vec has no prefix of its own; each
    // variable-size element still carries its member prefix (`auto`).
    let inner = VecDeque::from(vec![1i32, 2, 3]);
    let outer = vec![inner.clone(), VecDeque::from(vec![4i32])];
    let bytes = to_vec(&outer).unwrap();
    assert_eq!(from_slice::<Vec<VecDeque<i32>>>(&bytes).unwrap(), outer);

    let maps = vec![BTreeMap::from([(1i32, 2i32)]), BTreeMap::new()];
    assert_eq!(
        from_slice::<Vec<BTreeMap<i32, i32>>>(&to_vec(&maps).unwrap()).unwrap(),
        maps
    );
}

#[test]
fn test_misaligned_payload_is_rejected() {
    // `from_slice` is raw: the whole slice is the payload, no length prefix.
    // Delegating to `Vec<T>::decode` keeps the fixed-length alignment check:
    // 5 bytes cannot be a whole number of 4-byte elements.
    let bytes = [1u8, 2, 3, 4, 5];
    assert_eq!(
        from_slice::<BTreeSet<i32>>(&bytes),
        Err(Error::MisalignedElements)
    );
    assert_eq!(
        from_slice::<VecDeque<i32>>(&bytes),
        Err(Error::MisalignedElements)
    );
    assert_eq!(
        from_slice::<BTreeMap<i16, i16>>(&bytes),
        Err(Error::MisalignedElements)
    );
}

#[test]
fn test_round_trip_all_container_shapes() {
    let set: BTreeSet<String> = ["a", "bb", "ccc"].into_iter().map(String::from).collect();
    assert_eq!(
        from_slice::<BTreeSet<String>>(&to_vec(&set).unwrap()).unwrap(),
        set
    );
    let list: LinkedList<String> = ["a", "bb"].into_iter().map(String::from).collect();
    assert_eq!(
        from_slice::<LinkedList<String>>(&to_vec(&list).unwrap()).unwrap(),
        list
    );
    let map = BTreeMap::from([("k".to_string(), vec![1i32, 2])]);
    assert_eq!(
        from_slice::<BTreeMap<String, Vec<i32>>>(&to_vec(&map).unwrap()).unwrap(),
        map
    );
    let cow: Cow<'_, str> = Cow::Borrowed("round trip");
    let expected: Cow<'static, str> = Cow::Owned("round trip".to_string());
    assert_eq!(
        from_slice::<Cow<'static, str>>(&to_vec(&cow).unwrap()).unwrap(),
        expected
    );
}
