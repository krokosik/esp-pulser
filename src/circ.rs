//! Tiny curcular buffer

#[derive(Copy, Clone, Debug)]
pub struct Circ<T, const COUNT: usize> {
    pub data: [T; COUNT],
    pub next: usize,
}

impl<T, const COUNT: usize> Circ<T, COUNT>
where
    T: Copy,
{
    pub fn new(zero: T) -> Self {
        Circ {
            data: [zero; COUNT],
            next: 0,
        }
    }

    pub fn add(&mut self, s: T) {
        self.data[self.next] = s;
        self.next = wrap_next::<COUNT>(self.next);
    }

    pub fn iter(&self) -> CircIter<T, COUNT> {
        CircIter {
            circ: self,
            idx: self.next,
            done: false,
        }
    }
}

impl<'a, T, const COUNT: usize> IntoIterator for &'a Circ<T, COUNT>
where
    T: Copy,
{
    type Item = T;

    type IntoIter = CircIter<'a, T, COUNT>;

    fn into_iter(self) -> Self::IntoIter {
        CircIter {
            circ: self,
            idx: self.next,
            done: false,
        }
    }
}

pub struct CircIter<'a, T, const COUNT: usize> {
    circ: &'a Circ<T, COUNT>,
    idx: usize,
    done: bool,
}

impl<'a, T, const COUNT: usize> Iterator for CircIter<'a, T, COUNT>
where
    T: Copy,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            None
        } else {
            let res = self.circ.data[self.idx];
            self.idx = wrap_next::<COUNT>(self.idx);
            if self.idx == self.circ.next {
                self.done = true;
            }
            Some(res)
        }
    }
}

#[inline]
pub fn wrap_next<const COUNT: usize>(n: usize) -> usize {
    let n1 = n + 1;
    if n1 >= COUNT {
        0
    } else {
        n1
    }
}
