//! Nombre complexe minimal (f32).

use std::ops::{Add, AddAssign, Mul, MulAssign, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Cpx {
    pub re: f32,
    pub im: f32,
}

impl Cpx {
    pub const ZERO: Cpx = Cpx { re: 0.0, im: 0.0 };

    #[inline]
    pub const fn new(re: f32, im: f32) -> Self {
        Self { re, im }
    }

    #[inline]
    pub fn from_polar(r: f32, theta: f32) -> Self {
        let (s, c) = theta.sin_cos();
        Self { re: r * c, im: r * s }
    }

    #[inline]
    pub fn conj(self) -> Self {
        Self { re: self.re, im: -self.im }
    }

    #[inline]
    pub fn norm_sqr(self) -> f32 {
        self.re * self.re + self.im * self.im
    }

    #[inline]
    pub fn abs(self) -> f32 {
        self.norm_sqr().sqrt()
    }

    #[inline]
    pub fn arg(self) -> f32 {
        self.im.atan2(self.re)
    }

    /// Rotation de k·90° dans le sens HORAIRE (convention V.34).
    #[inline]
    pub fn rot_cw(self, k: u32) -> Self {
        match k & 3 {
            0 => self,
            1 => Cpx::new(self.im, -self.re),
            2 => Cpx::new(-self.re, -self.im),
            _ => Cpx::new(-self.im, self.re),
        }
    }

    /// Rotation de k·90° dans le sens anti-horaire.
    #[inline]
    pub fn rot_ccw(self, k: u32) -> Self {
        self.rot_cw((4 - (k & 3)) & 3)
    }
}

impl Add for Cpx {
    type Output = Cpx;
    #[inline]
    fn add(self, o: Cpx) -> Cpx {
        Cpx::new(self.re + o.re, self.im + o.im)
    }
}
impl Sub for Cpx {
    type Output = Cpx;
    #[inline]
    fn sub(self, o: Cpx) -> Cpx {
        Cpx::new(self.re - o.re, self.im - o.im)
    }
}
impl Mul for Cpx {
    type Output = Cpx;
    #[inline]
    fn mul(self, o: Cpx) -> Cpx {
        Cpx::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}
impl Mul<f32> for Cpx {
    type Output = Cpx;
    #[inline]
    fn mul(self, k: f32) -> Cpx {
        Cpx::new(self.re * k, self.im * k)
    }
}
impl Neg for Cpx {
    type Output = Cpx;
    #[inline]
    fn neg(self) -> Cpx {
        Cpx::new(-self.re, -self.im)
    }
}
impl AddAssign for Cpx {
    #[inline]
    fn add_assign(&mut self, o: Cpx) {
        self.re += o.re;
        self.im += o.im;
    }
}
impl SubAssign for Cpx {
    #[inline]
    fn sub_assign(&mut self, o: Cpx) {
        self.re -= o.re;
        self.im -= o.im;
    }
}
impl MulAssign<f32> for Cpx {
    #[inline]
    fn mul_assign(&mut self, k: f32) {
        self.re *= k;
        self.im *= k;
    }
}
