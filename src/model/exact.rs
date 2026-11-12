//! Exact arithmetic for the places where rounding a division would change
//! an answer: a fraction of two whole numbers of any size, never negative,
//! and the three ways one goes back to a `Decimal`: floored or ceiled onto
//! a grid, or rounded half to even at a number of places.
//!
//! A `Decimal` holds 96 bits, and its division rounds. Risk sizing
//! multiplies several amounts before it divides, and floors a quotient
//! whose exact value decides the quantity, so it works here and rounds
//! only where a rule says to. The whole numbers grow as they need to.
//! This is written by hand and kept small on purpose: schoolbook
//! multiplication, and long division a bit at a time, are plenty for the
//! handful of operations one sizing takes.

use rust_decimal::Decimal;
use std::cmp::Ordering;

/// A whole number, zero or more: 32-bit limbs, the least significant first,
/// with no zero limb at the end.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Natural(Vec<u32>);

impl Natural {
    fn of(value: u128) -> Self {
        let mut limbs = Vec::new();
        let mut rest = value;
        while rest > 0 {
            limbs.push(rest as u32);
            rest >>= 32;
        }
        Self(limbs)
    }

    fn power_of_ten(exponent: u32) -> Self {
        let ten = Self::of(10);
        (0..exponent).fold(Self::of(1), |power, _| power.times(&ten))
    }

    fn trimmed(mut limbs: Vec<u32>) -> Self {
        while limbs.last() == Some(&0) {
            limbs.pop();
        }
        Self(limbs)
    }

    fn is_zero(&self) -> bool {
        self.0.is_empty()
    }

    fn limb(&self, index: usize) -> u32 {
        self.0.get(index).copied().unwrap_or(0)
    }

    fn bits(&self) -> usize {
        match self.0.last() {
            Some(top) => self.0.len() * 32 - top.leading_zeros() as usize,
            None => 0,
        }
    }

    fn bit(&self, index: usize) -> bool {
        (self.limb(index / 32) >> (index % 32)) & 1 == 1
    }

    fn plus(&self, other: &Self) -> Self {
        let length = self.0.len().max(other.0.len());
        let mut limbs = Vec::with_capacity(length + 1);
        let mut carry = 0u64;
        for index in 0..length {
            let sum = carry + u64::from(self.limb(index)) + u64::from(other.limb(index));
            limbs.push(sum as u32);
            carry = sum >> 32;
        }
        limbs.push(carry as u32);
        Self::trimmed(limbs)
    }

    /// Takes `other` away. `other` is never larger than `self` where this
    /// is called.
    fn take(&mut self, other: &Self) {
        let mut borrow = 0u64;
        for (index, limb) in self.0.iter_mut().enumerate() {
            let taken = u64::from(other.limb(index)) + borrow;
            let held = u64::from(*limb);
            borrow = u64::from(held < taken);
            *limb = (held + (borrow << 32) - taken) as u32;
        }
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
    }

    fn times(&self, other: &Self) -> Self {
        let mut limbs = vec![0u32; self.0.len() + other.0.len()];
        for (low, &left) in self.0.iter().enumerate() {
            let mut carry = 0u64;
            for (offset, &right) in other.0.iter().enumerate() {
                let sum =
                    u64::from(limbs[low + offset]) + u64::from(left) * u64::from(right) + carry;
                limbs[low + offset] = sum as u32;
                carry = sum >> 32;
            }
            limbs[low + other.0.len()] = carry as u32;
        }
        Self::trimmed(limbs)
    }

    /// The quotient and the remainder of `self ÷ divisor`, or `None` for a
    /// divisor of zero.
    fn divided_by(&self, divisor: &Self) -> Option<(Self, Self)> {
        if divisor.is_zero() {
            return None;
        }
        let mut quotient = vec![0u32; self.0.len()];
        let mut remainder = Self(Vec::new());
        for index in (0..self.bits()).rev() {
            remainder.double_and_add(self.bit(index));
            if remainder.cmp(divisor) != Ordering::Less {
                remainder.take(divisor);
                quotient[index / 32] |= 1 << (index % 32);
            }
        }
        Some((Self::trimmed(quotient), remainder))
    }

    fn double_and_add(&mut self, bit: bool) {
        let mut carry = u32::from(bit);
        for limb in &mut self.0 {
            let top = *limb >> 31;
            *limb = (*limb << 1) | carry;
            carry = top;
        }
        if carry > 0 {
            self.0.push(carry);
        }
    }

    /// `self ÷ 10` where that is whole.
    fn tenth(&self) -> Option<Self> {
        let mut limbs = vec![0u32; self.0.len()];
        let mut remainder = 0u64;
        for index in (0..self.0.len()).rev() {
            let held = (remainder << 32) | u64::from(self.0[index]);
            limbs[index] = (held / 10) as u32;
            remainder = held % 10;
        }
        (remainder == 0).then(|| Self::trimmed(limbs))
    }

    /// The number as a `Decimal`'s mantissa at `scale`, with the trailing
    /// zeros that scale leaves it dropped, where one holds it.
    fn shortest(&self, mut scale: u32) -> Option<Decimal> {
        let mut digits = self.clone();
        while scale > 0 {
            let Some(shorter) = digits.tenth() else { break };
            digits = shorter;
            scale -= 1;
        }
        digits.decimal(scale)
    }

    /// The number as a `Decimal`'s mantissa at `scale`, where one holds it.
    fn decimal(&self, scale: u32) -> Option<Decimal> {
        if self.0.len() > 3 {
            return None;
        }
        let mantissa = (0..3).fold(0i128, |sum, index| {
            sum | (i128::from(self.limb(index)) << (32 * index))
        });
        Decimal::try_from_i128_with_scale(mantissa, scale).ok()
    }
}

impl Ord for Natural {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.iter().rev().cmp(other.0.iter().rev()))
    }
}

impl PartialOrd for Natural {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A fraction of two whole numbers, zero or more, kept exactly.
#[derive(Clone, Debug)]
pub(crate) struct Exact {
    numerator: Natural,
    /// Never zero.
    denominator: Natural,
}

impl Exact {
    /// The size of `value`, whatever its sign: nothing here is negative,
    /// so a caller that reads a signed amount carries the sign itself.
    pub(crate) fn magnitude(value: Decimal) -> Self {
        Self {
            numerator: Natural::of(value.mantissa().unsigned_abs()),
            denominator: Natural::power_of_ten(value.scale()),
        }
    }

    pub(crate) fn whole(value: u32) -> Self {
        Self {
            numerator: Natural::of(u128::from(value)),
            denominator: Natural::of(1),
        }
    }

    pub(crate) fn is_zero(&self) -> bool {
        self.numerator.is_zero()
    }

    pub(crate) fn times(&self, other: &Self) -> Self {
        Self {
            numerator: self.numerator.times(&other.numerator),
            denominator: self.denominator.times(&other.denominator),
        }
    }

    /// `self ÷ other`, or `None` where `other` is zero.
    pub(crate) fn over(&self, other: &Self) -> Option<Self> {
        (!other.is_zero()).then(|| Self {
            numerator: self.numerator.times(&other.denominator),
            denominator: self.denominator.times(&other.numerator),
        })
    }

    pub(crate) fn plus(&self, other: &Self) -> Self {
        let (left, right) = self.aligned(other);
        Self {
            numerator: left.plus(&right),
            denominator: self.denominator.times(&other.denominator),
        }
    }

    /// How far apart the two are: the larger less the smaller.
    pub(crate) fn distance(&self, other: &Self) -> Self {
        let (left, right) = self.aligned(other);
        let (mut more, less) = if left < right {
            (right, left)
        } else {
            (left, right)
        };
        more.take(&less);
        Self {
            numerator: more,
            denominator: self.denominator.times(&other.denominator),
        }
    }

    /// Both numerators over the product of the denominators.
    fn aligned(&self, other: &Self) -> (Natural, Natural) {
        (
            self.numerator.times(&other.denominator),
            other.numerator.times(&self.denominator),
        )
    }

    /// The largest multiple of `grid` at or below the value, in the grid's
    /// own scale. Where no `Decimal` holds it at that scale, as a grid
    /// written with more places than it needs can make it, its trailing
    /// zeros are dropped. `None` where the grid is not above zero, or where
    /// no `Decimal` holds the answer at all.
    pub(crate) fn floor_on(&self, grid: Decimal) -> Option<Decimal> {
        self.on_grid(grid, false)
    }

    /// The smallest multiple of `grid` at or above the value.
    pub(crate) fn ceil_on(&self, grid: Decimal) -> Option<Decimal> {
        self.on_grid(grid, true)
    }

    fn on_grid(&self, grid: Decimal, up: bool) -> Option<Decimal> {
        if grid.is_sign_negative() {
            return None;
        }
        let steps = self.over(&Self::magnitude(grid))?;
        let (mut count, rest) = steps.numerator.divided_by(&steps.denominator)?;
        if up && !rest.is_zero() {
            count = count.plus(&Natural::of(1));
        }
        let on_grid = count.times(&Natural::of(grid.mantissa().unsigned_abs()));
        on_grid
            .decimal(grid.scale())
            .or_else(|| on_grid.shortest(grid.scale()))
    }

    /// The value rounded half to even at `places`, with trailing zeros
    /// dropped: exact where it has no more places than that. `None` where
    /// no `Decimal` holds the answer.
    pub(crate) fn rounded(&self, places: u32) -> Option<Decimal> {
        let scaled = self.numerator.times(&Natural::power_of_ten(places));
        let (mut count, rest) = scaled.divided_by(&self.denominator)?;
        let up = match rest.plus(&rest).cmp(&self.denominator) {
            Ordering::Greater => true,
            Ordering::Equal => count.bit(0),
            Ordering::Less => false,
        };
        if up {
            count = count.plus(&Natural::of(1));
        }
        count.shortest(places)
    }
}

impl Ord for Exact {
    fn cmp(&self, other: &Self) -> Ordering {
        let (left, right) = self.aligned(other);
        left.cmp(&right)
    }
}

impl PartialOrd for Exact {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Exact {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Exact {}

#[cfg(test)]
mod tests;
