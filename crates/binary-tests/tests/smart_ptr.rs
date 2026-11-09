//! `Box<T>` / `Rc<T>` / `Arc<T>` are wire-transparent: the bytes are exactly
//! those of `T`, in raw, prefixed, and one-shot (also raw) positions. See
//! `binary::smart_ptr` (module docs) and `DESIGN.md` §9 / §13 / §17.3.

use binary::{from_slice, to_vec, write_named_member, Decode, Encode, Encoder, Layout};
use std::rc::Rc;
use std::sync::Arc;

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
fn test_box_matches_inner_in_raw_and_oneshot() {
    let inner = 0x0102_0304i32;
    let boxed = Box::new(inner);
    assert_eq!(raw(&boxed), raw(&inner));
    assert_eq!(to_vec(&boxed).unwrap(), to_vec(&inner).unwrap());
    assert_eq!(
        from_slice::<Box<i32>>(&to_vec(&inner).unwrap()).unwrap(),
        boxed
    );
}

#[test]
fn test_rc_arc_match_inner_in_raw_and_oneshot() {
    let inner = 0x0102_0304i32;
    let rc = Rc::new(inner);
    let arc = Arc::new(inner);
    let expected_raw = raw(&inner);

    // `to_vec` is raw as well (DESIGN §4), so the one-shot bytes equal `raw`;
    // keep both assertions to pin wire transparency at the top-level entry.
    let expected_oneshot = to_vec(&inner).unwrap();
    assert_eq!(expected_oneshot, expected_raw);

    assert_eq!(raw(&rc), expected_raw);
    assert_eq!(raw(&arc), expected_raw);
    assert_eq!(to_vec(&rc).unwrap(), expected_oneshot);
    assert_eq!(to_vec(&arc).unwrap(), expected_oneshot);
    assert_eq!(from_slice::<Rc<i32>>(&expected_oneshot).unwrap(), rc);
    assert_eq!(from_slice::<Arc<i32>>(&expected_oneshot).unwrap(), arc);
}

#[test]
fn test_smart_ptrs_in_containers_match_inner_containers() {
    // LEN is inherited, so `Vec<Ptr<i32>>` still sees fixed 4-byte elements
    // and writes no per-item prefix. The pointer keeps the default per-element
    // hooks, so this takes the loop — a code-path difference, never a wire
    // difference.
    let v = vec![1i32, -2, 3];
    let expected = to_vec(&v).unwrap();
    let b: Vec<Box<i32>> = v.iter().copied().map(Box::new).collect();
    let r: Vec<Rc<i32>> = v.iter().copied().map(Rc::new).collect();
    let a: Vec<Arc<i32>> = v.iter().copied().map(Arc::new).collect();

    assert_eq!(to_vec(&b).unwrap(), expected);
    assert_eq!(to_vec(&r).unwrap(), expected);
    assert_eq!(to_vec(&a).unwrap(), expected);
    assert_eq!(from_slice::<Vec<Box<i32>>>(&expected).unwrap(), b);
    assert_eq!(from_slice::<Vec<Rc<i32>>>(&expected).unwrap(), r);
    assert_eq!(from_slice::<Vec<Arc<i32>>>(&expected).unwrap(), a);
}

#[test]
fn test_smart_ptrs_forward_encode_auto_overrides() {
    // Tuples override encode_auto / decode_auto to flatten every member.
    // Default dispatch on LEN == 0 would add a spurious prefix here.
    let t = (7i32, "abc".to_string());
    let expected = to_vec(&t).unwrap();
    let b = Box::new(t.clone());
    let r = Rc::new(t.clone());
    let a = Arc::new(t.clone());

    assert_eq!(to_vec(&b).unwrap(), expected);
    assert_eq!(to_vec(&r).unwrap(), expected);
    assert_eq!(to_vec(&a).unwrap(), expected);
    assert_eq!(from_slice::<Box<(i32, String)>>(&expected).unwrap(), b);
    assert_eq!(from_slice::<Rc<(i32, String)>>(&expected).unwrap(), r);
    assert_eq!(from_slice::<Arc<(i32, String)>>(&expected).unwrap(), a);
}

#[test]
fn test_smart_ptrs_forward_encode_with_prefix_overrides() {
    // `write_named_member` always prefixes; `String` has an O(1) write_prefixed_bounded
    // override that a default-method forward would lose (`write_prefixed` fallback).
    let inner = String::from("abc");
    let expected = named_bytes(&inner);
    assert_eq!(named_bytes(&Box::new(inner.clone())), expected);
    assert_eq!(named_bytes(&Rc::new(inner.clone())), expected);
    assert_eq!(named_bytes(&Arc::new(inner)), expected);
}

#[test]
fn test_layout_is_inherited_through_pointers() {
    assert_eq!(<Box<i32> as Layout>::LEN, 4);
    assert_eq!(<Rc<i32> as Layout>::LEN, 4);
    assert_eq!(<Arc<i32> as Layout>::LEN, 4);
    assert_eq!(<Box<str> as Layout>::LEN, 0);
    assert_eq!(<Rc<[u8]> as Layout>::LEN, 0);

    // `LEN == 4` puts `Vec<Box<i32>>` in the fixed-size container branch, and
    // the pointer does *not* override the bulk hooks, so both directions still
    // go through `Decode` / `encode`. Wire equality with `Vec<i32>` is the
    // observable half; a bulk arm would have written pointer bytes instead.
    let boxes = vec![Box::new(1i32), Box::new(-2)];
    let bytes = raw(&boxes);
    assert_eq!(bytes, raw(&vec![1i32, -2]));
    assert_eq!(from_slice::<Vec<Box<i32>>>(&bytes).unwrap(), boxes);
}

#[test]
fn test_unsized_pointees_encode_like_borrowed_values() {
    let borrowed = to_vec(&"hello").unwrap();
    let bs: Box<str> = "hello".into();
    let rs: Rc<str> = "hello".into();
    let as_: Arc<str> = "hello".into();
    assert_eq!(to_vec(&bs).unwrap(), borrowed);
    assert_eq!(to_vec(&rs).unwrap(), borrowed);
    assert_eq!(to_vec(&as_).unwrap(), borrowed);

    let bytes: Box<[u8]> = vec![1u8, 2, 3].into_boxed_slice();
    let rb: Rc<[u8]> = Rc::from(&[1u8, 2, 3][..]);
    let ab: Arc<[u8]> = Arc::from(&[1u8, 2, 3][..]);
    assert_eq!(raw(&bytes), vec![1, 2, 3]);
    assert_eq!(raw(&rb), vec![1, 2, 3]);
    assert_eq!(raw(&ab), vec![1, 2, 3]);
}

#[test]
fn test_nested_pointers_are_transparent() {
    let expected = to_vec(&42i32).unwrap();
    let nested = Box::new(Box::new(42i32));
    assert_eq!(to_vec(&nested).unwrap(), expected);
    assert_eq!(from_slice::<Box<Box<i32>>>(&expected).unwrap(), nested);

    // Keep `Arc`'s pointee Send + Sync; wrapping an `Rc` in `Arc` is a real
    // API smell that clippy's `arc_with_non_send_sync` rejects.
    let mixed: Box<Rc<Arc<i32>>> = Box::new(Rc::new(Arc::new(42i32)));
    assert_eq!(to_vec(&mixed).unwrap(), expected);
    assert_eq!(from_slice::<Box<Rc<Arc<i32>>>>(&expected).unwrap(), mixed);
}

#[derive(Debug, PartialEq, Eq, Encode, Decode)]
struct Node {
    value: i32,
    next: Option<Box<Node>>,
}

#[test]
fn test_recursive_named_struct_round_trip() {
    let n = Node {
        value: 1,
        next: Some(Box::new(Node {
            value: 2,
            next: Some(Box::new(Node {
                value: 3,
                next: None,
            })),
        })),
    };
    let bytes = to_vec(&n).unwrap();
    assert_eq!(from_slice::<Node>(&bytes).unwrap(), n);
    assert_eq!(from_slice::<Box<Node>>(&bytes).unwrap(), Box::new(n));
}

/// §16.4 used to list recursive unions as blocked on `Box<T>`.
#[derive(Debug, PartialEq, Eq, Encode, Decode)]
#[binary(union)]
enum Tree {
    Leaf(i32),
    Node(Box<Tree>, Box<Tree>),
}

#[test]
fn test_recursive_union_round_trip() {
    let t = Tree::Node(
        Box::new(Tree::Leaf(1)),
        Box::new(Tree::Node(Box::new(Tree::Leaf(2)), Box::new(Tree::Leaf(3)))),
    );
    let bytes = to_vec(&t).unwrap();
    assert_eq!(from_slice::<Tree>(&bytes).unwrap(), t);
    assert_eq!(from_slice::<Rc<Tree>>(&bytes).unwrap(), Rc::new(t));
}

#[test]
fn test_recursive_tuple_layout_terminates() {
    // gen_layout_tuple sums <FieldTy as Layout>::LEN; Option<T>::LEN == 0
    // short-circuits, so const evaluation must not recurse forever.
    #[derive(Debug, PartialEq, Eq, Encode, Decode)]
    #[binary(tuple)]
    struct Linked {
        head: i32,
        tail: Option<Box<Linked>>,
    }
    assert_eq!(<Linked as Layout>::LEN, 0);
    let v = Linked {
        head: 9,
        tail: None,
    };
    let bytes = to_vec(&v).unwrap();
    assert_eq!(from_slice::<Linked>(&bytes).unwrap(), v);
    assert_eq!(
        raw(&v),
        raw(&Box::new(Linked {
            head: 9,
            tail: None,
        }))
    );
}

#[test]
fn test_decode_does_not_preserve_shared_ownership() {
    // The wire carries values, not object identity: clones of one pointer
    // decode into independent allocations. C# / C++ behave the same way.
    let shared = Rc::new(7i32);
    let bytes = to_vec(&(shared.clone(), shared.clone())).unwrap();
    let decoded: (Rc<i32>, Rc<i32>) = from_slice(&bytes).unwrap();
    assert_eq!(decoded.0, decoded.1);
    assert!(!Rc::ptr_eq(&decoded.0, &decoded.1));

    let shared = Arc::new(7i32);
    let bytes = to_vec(&(shared.clone(), shared.clone())).unwrap();
    let decoded: (Arc<i32>, Arc<i32>) = from_slice(&bytes).unwrap();
    assert_eq!(decoded.0, decoded.1);
    assert!(!Arc::ptr_eq(&decoded.0, &decoded.1));
}
