use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn normalize(self) -> Self {
        let mut rect = self;
        if rect.width < 0.0 {
            rect.x += rect.width;
            rect.width = -rect.width;
        }
        if rect.height < 0.0 {
            rect.y += rect.height;
            rect.height = -rect.height;
        }
        rect
    }

    pub fn max_x(self) -> f64 {
        self.x + self.width
    }

    pub fn max_y(self) -> f64 {
        self.y + self.height
    }

    pub fn intersects(self, other: &Rect) -> bool {
        let left = self.x.max(other.x);
        let right = self.max_x().min(other.max_x());
        let bottom = self.y.max(other.y);
        let top = self.max_y().min(other.max_y());
        left < right && bottom < top
    }

    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x && point.x <= self.max_x() && point.y >= self.y && point.y <= self.max_y()
    }

    pub fn union(self, other: &Rect) -> Rect {
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        let max_x = self.max_x().max(other.max_x());
        let max_y = self.max_y().max(other.max_y());
        Rect {
            x,
            y,
            width: max_x - x,
            height: max_y - y,
        }
    }

    pub fn to_quad(self) -> Quad {
        let rect = self.normalize();
        Quad {
            points: [
                Point::new(rect.x, rect.y),
                Point::new(rect.max_x(), rect.y),
                Point::new(rect.max_x(), rect.max_y()),
                Point::new(rect.x, rect.max_y()),
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Quad {
    pub points: [Point; 4],
}

impl Quad {
    pub fn bounding_rect(self) -> Rect {
        let mut min_x = f64::INFINITY;
        let mut min_y = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut max_y = f64::NEG_INFINITY;
        for point in self.points {
            min_x = min_x.min(point.x);
            min_y = min_y.min(point.y);
            max_x = max_x.max(point.x);
            max_y = max_y.max(point.y);
        }
        Rect {
            x: min_x,
            y: min_y,
            width: max_x - min_x,
            height: max_y - min_y,
        }
    }

    pub fn intersects_rect(self, rect: &Rect) -> bool {
        self.bounding_rect().intersects(rect)
    }

    pub fn intersects_quad(self, other: &Quad) -> bool {
        self.bounding_rect().intersects(&other.bounding_rect())
    }

    pub fn transform(self, matrix: Matrix) -> Self {
        Quad {
            points: self.points.map(|point| matrix.transform_point(point)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const fn identity() -> Self {
        Self {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn multiply(self, other: Matrix) -> Matrix {
        Matrix {
            a: self.a * other.a + self.b * other.c,
            b: self.a * other.b + self.b * other.d,
            c: self.c * other.a + self.d * other.c,
            d: self.c * other.b + self.d * other.d,
            e: self.e * other.a + self.f * other.c + other.e,
            f: self.e * other.b + self.f * other.d + other.f,
        }
    }

    pub fn translate(tx: f64, ty: f64) -> Matrix {
        Matrix {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: tx,
            f: ty,
        }
    }

    pub fn scale(sx: f64, sy: f64) -> Matrix {
        Matrix {
            a: sx,
            b: 0.0,
            c: 0.0,
            d: sy,
            e: 0.0,
            f: 0.0,
        }
    }

    pub fn rotate_degrees(angle: i32) -> Matrix {
        match angle.rem_euclid(360) {
            90 => Matrix {
                a: 0.0,
                b: 1.0,
                c: -1.0,
                d: 0.0,
                e: 0.0,
                f: 0.0,
            },
            180 => Matrix {
                a: -1.0,
                b: 0.0,
                c: 0.0,
                d: -1.0,
                e: 0.0,
                f: 0.0,
            },
            270 => Matrix {
                a: 0.0,
                b: -1.0,
                c: 1.0,
                d: 0.0,
                e: 0.0,
                f: 0.0,
            },
            _ => Matrix::identity(),
        }
    }

    pub fn transform_point(self, point: Point) -> Point {
        Point {
            x: point.x * self.a + point.y * self.c + self.e,
            y: point.x * self.b + point.y * self.d + self.f,
        }
    }

    pub fn transform_rect(self, rect: Rect) -> Rect {
        rect.to_quad().transform(self).bounding_rect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageBox {
    pub media_box: Rect,
    pub crop_box: Rect,
    pub rotate: i32,
}

impl PageBox {
    pub fn normalized_transform(self) -> Matrix {
        let media = self.crop_box.normalize();
        let translate = Matrix::translate(-media.x, -media.y);
        let rotate = Matrix::rotate_degrees(self.rotate);
        let rotated = rotate.transform_rect(Rect {
            x: 0.0,
            y: 0.0,
            width: media.width,
            height: media.height,
        });
        let fixup = Matrix::translate(-rotated.x, -rotated.y);
        translate.multiply(rotate).multiply(fixup)
    }

    pub fn size(self) -> Size {
        let rect = self.normalized_transform().transform_rect(self.crop_box);
        Size {
            width: rect.width,
            height: rect.height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const BLACK: Color = Color { r: 0, g: 0, b: 0 };
}

#[cfg(test)]
mod tests {
    use super::{Matrix, Point, Rect};

    #[test]
    fn rect_normalization_flips_negative_dimensions() {
        let normalized = Rect {
            x: 10.0,
            y: 20.0,
            width: -4.0,
            height: -2.0,
        }
        .normalize();
        assert_eq!(
            normalized,
            Rect {
                x: 6.0,
                y: 18.0,
                width: 4.0,
                height: 2.0,
            }
        );
    }

    #[test]
    fn matrix_transforms_points() {
        let matrix = Matrix::translate(10.0, 5.0).multiply(Matrix::scale(2.0, 3.0));
        let point = matrix.transform_point(Point::new(4.0, 2.0));
        assert_eq!(point, Point::new(28.0, 21.0));
    }
}
