// Copyright 2016 The Servo Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::borrow::Borrow;
use std::cell::UnsafeCell;
use std::cmp::Ordering::{self, Less, Equal, Greater};
use std::default::Default;
use std::iter::{FromIterator, IntoIterator};
use std::mem;
use std::ops::{Index, IndexMut};

use super::node::Node;

/// The implementation of this splay tree is largely based on the c code at:
///     ftp://ftp.cs.cmu.edu/usr/ftp/usr/sleator/splaying/top-down-splay.c
/// This version of splaying is a top-down splay operation.
pub struct SplayMap<K: Ord, V> {
    root: UnsafeCell<Option<Box<Node<K, V>>>>,
    size: usize,
}

pub struct IntoIter<K, V> {
    cur: Option<Box<Node<K, V>>>,
    remaining: usize,
}

/// Performs a top-down splay operation on a tree rooted at `node`. This will
/// modify the pointer to contain the new root of the tree once the splay
/// operation is done. When finished, if `key` is in the tree, it will be at the
/// root. Otherwise the closest key to the specified key will be at the root.
fn splay_with<K, V, Q>(mut compare: Q, node: &mut Box<Node<K, V>>)
                       where K: Ord,
                             Q: FnMut(&K, &V) -> Ordering {
    let mut newleft = None;
    let mut newright = None;

    // Explicitly grab a new scope so the loans on newleft/newright are
    // terminated before we move out of them.
    {
        // Yes, these are backwards, that's intentional.
        let mut l = &mut newright;
        let mut r = &mut newleft;

        loop {
            match compare(&node.key_value.0, &node.key_value.1) {
                // Found it, yay!
                Equal => { break }

                Less => {
                    if node.left.is_none() {
                        break
                    }
                    let mut left = node.take_left();
                    // rotate this node right if necessary
                    if compare(&left.key_value.0, &left.key_value.1) == Less {
                        // A bit odd, but avoids drop glue
                        mem::swap(&mut node.left, &mut left.right);
                        mem::swap(&mut left, node);
                        let none = mem::replace(&mut node.right, Some(left));
                        match mem::replace(&mut node.left, none) {
                            None => break,
                            Some(l) => left = l,
                        }
                    }

                    mem::forget(mem::replace(r, Some(mem::replace(node, left))));
                    let tmp = r;
                    r = &mut tmp.as_mut().unwrap().left;
                }

                // If you look closely, you may have seen some similar code
                // before
                Greater => {
                    if node.right.is_none() {
                        break
                    }
                    let mut right = node.take_right();
                    // Rotate right if necessary.
                    if compare(&right.key_value.0, &right.key_value.1) == Greater {
                        mem::swap(&mut node.right, &mut right.left);
                        mem::swap(&mut right, node);
                        let none = mem::replace(&mut node.left, Some(right));
                        match mem::replace(&mut node.right, none) {
                            None => break,
                            Some(r) => right = r,
                        }
                    }
                    mem::forget(mem::replace(l, Some(mem::replace(node, right))));
                    let tmp = l;
                    l = &mut tmp.as_mut().unwrap().right;
                }
            }
        }

        mem::swap(l, &mut node.left);
        mem::swap(r, &mut node.right);
    }

    // Optimization to avoid drop glue…
    mem::forget(mem::replace(&mut node.left, newright));
    mem::forget(mem::replace(&mut node.right, newleft));
}

fn splay_with_key<K, V, Q: ?Sized>(key: &Q, node: &mut Box<Node<K, V>>)
                                   where K: Ord + Borrow<Q>, Q: Ord {
    splay_with(|other_key, _| key.cmp(other_key.borrow()), node)
}

fn lower_bound_with<K, V, Q>(mut compare: Q, node: &Box<Node<K, V>>) -> Option<&(K, V)>
                             where K: Ord, Q: FnMut(&K, &V) -> Ordering {
    match compare(&node.key_value.0, &node.key_value.1) {
        Less => {
            if let Some(ref left) = node.left {
                if let Some(key_value) = lower_bound_with(compare, left) {
                    return Some(key_value)
                }
            }
            Some(&node.key_value)
        }
        Greater => {
            match node.right {
                Some(ref right) => lower_bound_with(compare, right),
                None => None,
            }
        }
        Equal => Some(&node.key_value),
    }
}

impl<K: Ord, V> SplayMap<K, V> {
    pub fn new() -> SplayMap<K, V> {
        SplayMap { root: UnsafeCell::new(None), size: 0 }
    }

    /// Moves all values out of this map, transferring ownership to the given
    /// iterator.
    pub fn into_iter(mut self) -> IntoIter<K, V> {
        IntoIter { cur: self.root_mut().take(), remaining: self.size }
    }

    /// Clears the tree in O(1) extra space (including the stack). This is
    /// necessary to prevent stack exhaustion with extremely large trees.
    pub fn clear(&mut self) {
        let iter = IntoIter {
            cur: self.root_mut().take(),
            remaining: self.size,
        };
        for _ in iter {
            // ignore, drop the values (and the node)
        }
        self.size = 0;
    }

    /// Return a reference to the value corresponding to the key
    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
        where K: Borrow<Q>, Q: Ord,
    {
        // Splay trees are self-modifying, so they can't exactly operate with
        // the immutable self given by the Map interface for this method. It can
        // be guaranteed, however, that the callers of this method are not
        // modifying the tree, they may just be rotating it. Each node is a
        // pointer on the heap, so we know that none of the pointers inside
        // these nodes (the value returned from this function) will be moving.
        //
        // With this in mind, we can unsafely use a mutable version of this tree
        // to invoke the splay operation and return a pointer to the inside of
        // one of the nodes (the pointer won't be deallocated or moved).
        //
        // However I'm not entirely sure whether this works with iteration or
        // not. Arbitrary lookups can occur during iteration, and during
        // iteration there's some form of "stack" remembering the nodes that
        // need to get visited. I don't believe that it's safe to allow lookups
        // while the tree is being iterated. Right now there are no iterators
        // exposed on this splay tree implementation, and more thought would be
        // required if there were.
        unsafe {
            match *self.root.get() {
                Some(ref mut root) => {
                    splay_with_key(key, root);
                    if key == root.key_value.0.borrow() {
                        return Some(&root.key_value.1);
                    }
                    None
                }
                None => None,
            }
        }
    }

    /// Return a mutable reference to the value corresponding to the key
    pub fn get_mut<Q: ?Sized>(&mut self, key: &Q) -> Option<&mut V>
        where K: Borrow<Q>, Q: Ord,
    {
        match *self.root_mut() {
            None => { return None; }
            Some(ref mut root) => {
                splay_with_key(key, root);
                if key == root.key_value.0.borrow() {
                    return Some(&mut root.key_value.1);
                }
                return None;
            }
        }
    }

    pub fn get_with_mut<Q>(&mut self, mut compare: Q) -> Option<&mut (K, V)>
                           where Q: FnMut(&K, &V) -> Ordering {
        match *self.root_mut() {
            None => None,
            Some(ref mut root) => {
                splay_with(&mut compare, root);
                if compare(&root.key_value.0, &root.key_value.1) == Equal {
                    Some(&mut root.key_value)
                } else {
                    None
                }
            }
        }
    }

    pub fn lower_bound_with<Q>(&self, compare: Q) -> Option<&(K, V)>
                               where Q: FnMut(&K, &V) -> Ordering {
        self.root_ref().as_ref().and_then(|root| lower_bound_with(compare, root))
    }

    /// Insert a key-value pair from the map. If the key already had a value
    /// present in the map, that value is returned. Otherwise None is returned.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        match self.root_mut() {
            &mut Some(ref mut root) => {
                splay_with_key(&key, root);

                match key.cmp(&root.key_value.0) {
                    Equal => {
                        let old = mem::replace(&mut root.key_value.1, value);
                        return Some(old);
                    }
                    /* TODO: would unsafety help perf here? */
                    Less => {
                        let left = root.pop_left();
                        let new = Node::new(key, value, left, None);
                        let prev = mem::replace(root, new);
                        root.right = Some(prev);
                    }
                    Greater => {
                        let right = root.pop_right();
                        let new = Node::new(key, value, None, right);
                        let prev = mem::replace(root, new);
                        root.left = Some(prev);
                    }
                }
            }
            slot => {
                *slot = Some(Node::new(key, value, None, None));
            }
        }
        self.size += 1;
        return None;
    }

    /// Removes a key from the map, returning the value at the key if the key
    /// was previously in the map.
    pub fn remove<Q: ?Sized>(&mut self, key: &Q) -> Option<V>
        where K: Borrow<Q>, Q: Ord
    {
        match *self.root_mut() {
            None => { return None; }
            Some(ref mut root) => {
                splay_with_key(key, root);
                if key != root.key_value.0.borrow() { return None }
            }
        }

        // TODO: Extra storage of None isn't necessary
        let (value, left, right) = match *self.root_mut().take().unwrap() {
            Node {key_value: (_, value), left, right} => (value, left, right)
        };

        *self.root_mut() = match left {
            None => right,
            Some(mut node) => {
                splay_with_key(key, &mut node);
                node.right = right;
                Some(node)
            }
        };

        self.size -= 1;
        return Some(value);
    }
}

impl<K: Ord, V> SplayMap<K, V> {
    // These two functions provide safe access to the root node, and they should
    // be valid to call in virtually all contexts.
    fn root_mut(&mut self) -> &mut Option<Box<Node<K, V>>> {
        unsafe { &mut *self.root.get() }
    }
    fn root_ref(&self) -> &Option<Box<Node<K, V>>> {
        unsafe { &*self.root.get() }
    }
}

impl<'a, K: Ord, V, Q: ?Sized> Index<&'a Q> for SplayMap<K, V>
    where K: Borrow<Q>, Q: Ord
{
    type Output = V;
    fn index(&self, index: &'a Q) -> &V {
        self.get(index).expect("key not present in SplayMap")
    }
}

impl<'a, K: Ord, V, Q: ?Sized> IndexMut<&'a Q> for SplayMap<K, V>
    where K: Borrow<Q>, Q: Ord
{
    fn index_mut(&mut self, index: &'a Q) -> &mut V {
        self.get_mut(index).expect("key not present in SplayMap")
    }
}

impl<K: Ord, V> Default for SplayMap<K, V> {
    fn default() -> SplayMap<K, V> { SplayMap::new() }
}

impl<K: Ord, V> FromIterator<(K, V)> for SplayMap<K, V> {
    fn from_iter<I: IntoIterator<Item=(K, V)>>(iterator: I) -> SplayMap<K, V> {
        let mut map = SplayMap::new();
        map.extend(iterator);
        map
    }
}

impl<K: Ord, V> Extend<(K, V)> for SplayMap<K, V> {
    fn extend<I: IntoIterator<Item=(K, V)>>(&mut self, i: I) {
        for (k, v) in i {
            self.insert(k, v);
        }
    }
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        let mut cur = match self.cur.take() {
            Some(cur) => cur,
            None => return None,
        };
        loop {
            match cur.pop_left() {
                Some(node) => {
                    let mut node = node;
                    cur.left = node.pop_right();
                    node.right = Some(cur);
                    cur = node;
                }

                None => {
                    self.cur = cur.pop_right();
                    // left and right fields are both None
                    let node = *cur;
                    let Node { key_value, .. } = node;
                    self.remaining -= 1;
                    return Some(key_value);
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<K, V> DoubleEndedIterator for IntoIter<K, V> {
    // Pretty much the same as the above code, but with left replaced with right
    // and vice-versa.
    fn next_back(&mut self) -> Option<(K, V)> {
        let mut cur = match self.cur.take() {
            Some(cur) => cur,
            None => return None,
        };
        loop {
            match cur.pop_right() {
                Some(node) => {
                    let mut node = node;
                    cur.right = node.pop_left();
                    node.left = Some(cur);
                    cur = node;
                }

                None => {
                    self.cur = cur.pop_left();
                    // left and right fields are both None
                    let node = *cur;
                    let Node { key_value, .. } = node;
                    self.remaining -= 1;
                    return Some(key_value);
                }
            }
        }
    }
}

impl<K, V> ExactSizeIterator for IntoIter<K, V> {}

impl<K: Clone + Ord, V: Clone> Clone for SplayMap<K, V> {
    fn clone(&self) -> SplayMap<K, V> {
        SplayMap {
            root: UnsafeCell::new(self.root_ref().clone()),
            size: self.size,
        }
    }
}

impl<K: Ord, V> Drop for SplayMap<K, V> {
    fn drop(&mut self) {
        // Be sure to not recurse too deep on destruction
        self.clear();
    }
}
