use map::SplayMap;
use std::cmp::Ordering;
use std::fmt::{self, Debug, Formatter};
use std::i32;
use std::iter;
use std::ops::{Add, Neg, Sub};

const MAX_AU: Au = Au(i32::MAX);

pub struct Exclusions {
    bands: SplayMap<Au, Band>,
    inline_size: Au,
}

#[derive(Clone, Copy, Debug)]
struct Band {
    left: Au,
    right: Au,
    length: Au,
}

impl Band {
    fn new(left: Au, right: Au, length: Au) -> Band {
        Band {
            left: left,
            right: right,
            length: length,
        }
    }

    fn available_size(&self, inline_size: Au) -> Au {
        inline_size + self.left + self.right
    }

    fn get(&self, side: Side) -> Au {
        match side {
            Side::Left => self.left,
            Side::Right => self.right,
        }
    }

    fn set(&mut self, side: Side, inline_size: Au) {
        match side {
            Side::Left => self.left = inline_size,
            Side::Right => self.right = inline_size,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub inline: Au,
    pub block: Au,
}

#[derive(Clone, Copy, Debug)]
pub struct Size {
    pub inline: Au,
    pub block: Au,
}

impl Size {
    pub fn new(inline: Au, block: Au) -> Size {
        Size {
            inline: inline,
            block: block,
        }
    }
}

#[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Ord, Debug)]
pub struct Au(pub i32);

impl Add<Au> for Au {
    type Output = Au;
    fn add(self, other: Au) -> Au {
        Au(self.0 + other.0)
    }
}

impl Sub<Au> for Au {
    type Output = Au;
    fn sub(self, other: Au) -> Au {
        Au(self.0 - other.0)
    }
}

impl Neg for Au {
    type Output = Au;
    fn neg(self) -> Au {
        Au(-self.0)
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Side {
    Left,
    Right,
}

impl Debug for Exclusions {
    fn fmt(&self, formatter: &mut Formatter) -> Result<(), fmt::Error> {
        try!(writeln!(formatter, "Exclusions(inline_size={:?}): bands:", self.inline_size));
        for (block_position, band) in self.bands.clone().into_iter() {
            try!(writeln!(formatter, "    {:?} {:?}", block_position, band));
        }
        Ok(())
    }
}

impl Exclusions {
    pub fn new(inline_size: Au) -> Exclusions {
        Exclusions {
            bands: iter::once((Au(0), Band::new(Au(0), Au(0), MAX_AU))).collect(),
            inline_size: inline_size,
        }
    }

    pub fn place(&mut self, side: Side, size: &Size) -> Point {
        //println!("place(side={:?}, size={:?}): {:?}", side, size, self);
        let block_position =
            self.bands
                .lower_bound_with(|&band_block_start, band| {
                    compare_inline_size(band_block_start, band, size, self.inline_size)
                }).expect("Exclusions::place(): Didn't find a band!").0;
        let band = self.bands.get(&block_position).unwrap();
        let inline_position = match side {
            Side::Left => -band.left,
            Side::Right => self.inline_size + band.right - size.inline,
        };
        let origin = Point {
            inline: inline_position,
            block: block_position,
        };
        //println!("... placed at {:?}", origin);
        origin
    }

    pub fn exclude(&mut self, side: Side, size: &Size) {
        //println!("exclude(side={:?}, size={:?}): {:?}", side, size, self);
        if size.inline == Au(0) || size.block == Au(0) {
            return
        }

        self.split(side, size);

        /*let (ceiling_block_position, ceiling_band) = {
            let inline_size = self.inline_size;
            let &mut (ceiling_block_position, ref mut ceiling_band) =
                self.bands
                    .lower_bound_with_mut(|&band_block_start, band| {
                        compare_inline_size(band_block_start, band, size, inline_size)
                    }).expect("Exclusions::exclude(): Didn't find the ceiling?!");
            ceiling_band.set(side, -size.inline);
            (ceiling_block_position, *ceiling_band)
        };*/

        let (mut last_block_position, mut last_band) = (size.block, None);
        loop {
            let (block_position, band) = match self.bands.get_with_mut(|block_position, band| {
                if last_block_position <= *block_position {
                    Ordering::Less
                } else if last_block_position > *block_position + band.length {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            }) {
                Some(&mut (block_position, ref mut band)) if -band.get(side) <= size.inline => {
                    band.set(side, -size.inline);
                    (block_position, *band)
                }
                Some(_) | None => break,
            };
            // TODO(pcwalton): Merge
            last_block_position = block_position;
            last_band = Some(band)
        }

        /*{
            let ceiling_band =
                self.bands
                    .get_mut(&ceiling_block_position)
                    .expect("Exclusions::exclude(): Didn't find the ceiling band!");
            if ceiling_block_position + ceiling_band.length != MAX_AU {
                ceiling_band.length = size.block - ceiling_block_position
            }
        }*/

        //println!("... exclude done: {:?}", self);
    }

    fn split(&mut self, side: Side, size: &Size) {
        //println!("split(side={:?}, size={:?}): {:?}", side, size, self);
        let (floor, left_size, right_size) = {
            let &mut (upper_block_position, ref mut upper_band) =
                self.bands.get_with_mut(|block_position, band| {
                    if size.block < *block_position {
                        Ordering::Less
                    } else if size.block >= *block_position + band.length {
                        Ordering::Greater
                    } else {
                        Ordering::Equal
                    }
                }).expect("Exclusions::split(): Didn't find band to split!");
            let floor = upper_block_position + upper_band.length;
            upper_band.length = size.block - upper_block_position;
            (floor, upper_band.left, upper_band.right)
        };
        let lower_band_length = floor - size.block;
        let lower_band = Band::new(left_size, right_size, floor - size.block);
        self.bands.insert(size.block, lower_band);
        //println!("... split done: {:?}", self);
    }
}

fn compare_inline_size(band_block_start: Au,
                       band: &Band,
                       exclusion_size: &Size,
                       inline_size: Au)
                       -> Ordering {
    match exclusion_size.inline.cmp(&band.available_size(inline_size)) {
        Ordering::Less | Ordering::Equal => Ordering::Less,
        Ordering::Greater if band_block_start + band.length == MAX_AU => Ordering::Equal,
        Ordering::Greater => Ordering::Greater,
    }
}


