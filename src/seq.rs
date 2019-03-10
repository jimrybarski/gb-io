use bio::alphabets::dna::revcomp;
use std::borrow::Cow;
use std::cmp;
use std::fmt;
use std::io;
use std::io::Write;
use std::str;

// use chrono::NaiveDate;

pub use crate::{FeatureKind, QualifierKey};

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
/// A very simple Date struct so we don't have to rely on chrono
pub struct Date {
    year: i32,
    month: u32,
    day: u32,
}

#[derive(Debug, PartialEq, Clone)]
pub struct DateError;

impl Date {
    /// Construct from a calendar date, checks that the numbers look
    /// reasonable but nothing too exhaustive
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Result<Date, DateError> {
        if month >= 1 && month <= 12 && day >= 1 && day <= 31 {
            Ok(Date { year, month, day })
        } else {
            Err(DateError)
        }
    }
    pub fn year(&self) -> i32 {
        self.year
    }
    pub fn month(&self) -> u32 {
        self.month
    }
    pub fn day(&self) -> u32 {
        self.day
    }
}

impl fmt::Display for Date {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let month = match self.month {
            1 => "JAN",
            2 => "FEB",
            3 => "MAR",
            4 => "APR",
            5 => "MAY",
            6 => "JUN",
            7 => "JUL",
            8 => "AUG",
            9 => "SEP",
            10 => "OCT",
            11 => "NOV",
            12 => "DEC",
            _ => unreachable!(),
        };
        write!(f, "{:02}-{}-{:04}", self.day, month, self.year)
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct Before(pub bool);
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct After(pub bool);

/// Represents a Genbank "position", used to specify the location of
/// features and in the CONTIG line. See
/// <http://www.insdc.org/files/feature_table.html> for a detailed
/// description of what they mean.
///
/// Note that positions specified here must always refer to a
/// nucleotide within the sequence. Ranges are inclusive. To specify a
/// range that wraps around, use join(x..last,1..y).
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub enum Position {
    /// Just a number
    Single(i64),
    /// n^n+1
    Between(i64, i64),
    /// [<]x..[>]y
    Span((i64, Before), (i64, After)),
    Complement(Box<Position>),
    Join(Vec<Position>),
    Order(Vec<Position>),
    Bond(Vec<Position>),
    OneOf(Vec<Position>),
    External(String, Option<Box<Position>>),
    Gap(Option<i64>),
}

#[derive(Debug, Fail, PartialEq)]
pub enum PositionError {
    #[fail(display = "Can't determine position due to ambiguity: {}", _0)]
    Ambiguous(Position),
    #[fail(display = "Can't resolve external position: {}", _0)]
    External(Position),
    #[fail(display = "Recursion limit reached while processing: {}", _0)]
    Recursion(Position),
    // TODO: actually implement this
    #[fail(display = "Empty position list encountered")]
    Empty,
    #[fail(
        display = "Position refers to a location outside of the sequence: {}",
        _0
    )]
    OutOfBounds(Position),
}

impl Position {
    /// Convenience constructor for this commonly used variant
    pub fn simple_span(a: i64, b: i64) -> Position {
        Position::Span((a, Before(false)), (b, After(false)))
    }

    /// Try to get the outermost bounds for a position. Returns the
    /// starting and finishing positions, as inclusive sequence
    /// coordinates.
    pub fn find_bounds(&self) -> Result<(i64, i64), PositionError> {
        match *self {
            Position::Span((a, _), (b, _)) => Ok((a, b)),
            Position::Complement(ref position) => position.find_bounds(),
            Position::Join(ref positions) => {
                let first = positions.first().ok_or(PositionError::Empty)?;
                let last = positions.last().unwrap();
                let (start, _) = first.find_bounds()?;
                let (_, end) = last.find_bounds()?;
                Ok((start, end))
            }
            Position::Single(p) => Ok((p, p)),
            Position::Between(a, b) => Ok((a, b)),
            Position::Order(ref positions)
            | Position::Bond(ref positions)
            | Position::OneOf(ref positions) => {
                let (left, right): (Vec<_>, Vec<_>) = positions
                    .iter()
                    .flat_map(Position::find_bounds) // This skips any Err values
                    .unzip();
                let min = left.into_iter().min().ok_or(PositionError::Empty)?;
                let max = right.into_iter().max().unwrap();
                Ok((min, max))
            }
            ref p => Err(PositionError::Ambiguous(p.clone())),
        }
    }

    // Only returns `Err` if one of the closures does
    fn transform<P, V>(self, pos: &P, val: &V) -> Result<Position, PositionError>
    where
        P: Fn(Position) -> Result<Position, PositionError>,
        V: Fn(i64) -> Result<i64, PositionError>,
    {
        pos(self)?.transform_impl(pos, val)
    }

    fn transform_impl<P, V>(self, pos: &P, val: &V) -> Result<Position, PositionError>
    where
        P: Fn(Position) -> Result<Position, PositionError>,
        V: Fn(i64) -> Result<i64, PositionError>,
    {
        use Position::*;
        let t_vec = |ps: Vec<Position>| {
            ps.into_iter()
                .map(|p| pos(p)?.transform_impl(pos, val))
                .collect::<Result<Vec<_>, _>>()
        };
        let res = match self {
            // Apply the position closure
            Complement(p) => Complement(Box::new(pos(*p)?.transform_impl(pos, val)?)),
            Order(ps) => Order(t_vec(ps)?),
            Bond(ps) => Bond(t_vec(ps)?),
            OneOf(ps) => OneOf(t_vec(ps)?),
            Join(ps) => Join(t_vec(ps)?),
            // Apply the value closure
            Single(v) => Single(val(v)?),
            Between(a, b) => Between(val(a)?, val(b)?),
            Span((a, before), (b, after)) => Span((val(a)?, before), (val(b)?, after)),
            // We don't touch values here
            External(..) => self,
            Gap(..) => self,
        };
        Ok(res)
    }

    /// Truncates this position, limiting it to the given bounds.
    /// Note: `higher` is exclusive.
    /// `None` is returned if no part of the position lies within the bounds.
    pub fn truncate(&self, start: i64, end: i64) -> Option<Position> {
        use Position::*;
        let filter = |ps: &[Position]| {
            let res: Vec<_> = ps.iter().filter_map(|p| p.truncate(start, end)).collect();
            if res.is_empty() {
                None
            } else {
                Some(res)
            }
        };
        match *self {
            Single(a) => {
                if a >= start && a < end {
                    Some(self.clone())
                } else {
                    None
                }
            }
            Between(a, b) => {
                if (a >= start && a < end) || (b >= start && b < end) {
                    Some(self.clone())
                } else {
                    None
                }
            }
            Span((a, before), (b, after)) => {
                match (a >= start && a < end, b >= start && b < end) {
                    (true, true) => Some(simplify_shallow(Span((a, before), (b, after))).unwrap()),
                    (true, false) => {
                        Some(simplify_shallow(Span((a, before), (end - 1, After(false)))).unwrap())
                    } // TODO: this should be true?
                    (false, true) => {
                        Some(simplify_shallow(Span((start, Before(false)), (b, after))).unwrap())
                    } // TODO
                    (false, false) => {
                        // does the position span (start, end)?
                        if a <= start && b >= end - 1 {
                            Some(simplify_shallow(Position::simple_span(start, end - 1)).unwrap()) // TODO
                        } else {
                            None
                        }
                    }
                }
            }
            Complement(ref a) => a.truncate(start, end).map(|a| Complement(Box::new(a))),
            Join(ref ps) => filter(ps).map(|v| simplify_shallow(Join(v)).unwrap()),
            OneOf(ref ps) => filter(ps).map(OneOf),
            Bond(ref ps) => filter(ps).map(Bond),
            Order(ref ps) => filter(ps).map(Order),
            External(_, _) | Gap(_) => Some(self.clone()),
        }
    }

    pub fn to_gb_format(&self) -> String {
        fn position_list(positions: &[Position]) -> String {
            positions
                .iter()
                .map(Position::to_gb_format)
                .collect::<Vec<_>>()
                .join(",")
        }
        match *self {
            Position::Single(p) => format!("{}", p + 1),
            Position::Between(a, b) => format!("{}^{}", a + 1, b + 1),
            Position::Span((a, Before(before)), (b, After(after))) => {
                let before = if before { "<" } else { "" };
                let after = if after { ">" } else { "" };
                format!("{}{}..{}{}", before, a + 1, after, b + 1)
            }
            Position::Complement(ref p) => format!("complement({})", p.to_gb_format()),
            Position::Join(ref positions) => format!("join({})", position_list(positions)),
            Position::Order(ref positions) => format!("order({})", position_list(positions)),
            Position::Bond(ref positions) => format!("bond({})", position_list(positions)),
            Position::OneOf(ref positions) => format!("one-of({})", position_list(positions)),
            Position::External(ref name, Some(ref pos)) => {
                format!("{}:{}", &name, pos.to_gb_format())
            }
            Position::External(ref name, None) => format!("{}", name),
            Position::Gap(Some(length)) => format!("gap({})", length),
            Position::Gap(None) => format!("gap()"),
        }
    }
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_gb_format())
    }
}

/// An annotation, or "feature". Note that one qualifier key can occur multiple
/// times with distinct values. We store them in a `Vec` to preserve order. Some
/// qualifiers have no value.
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct Feature {
    pub kind: FeatureKind,
    pub pos: Position,
    pub qualifiers: Vec<(QualifierKey, Option<String>)>,
}

impl Feature {
    /// Returns all the values for a given QualifierKey. Qualifiers with no
    /// value (ie. `/foo`) are ignored
    pub fn qualifier_values(&self, key: QualifierKey) -> impl Iterator<Item = &str> {
        self.qualifiers
            .iter()
            .filter(move |&(k, _)| k == &key)
            .filter_map(|&(_, ref v)| v.as_ref().map(String::as_str))
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub enum Topology {
    Linear,
    Circular,
}

impl fmt::Display for Topology {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let res = match *self {
            Topology::Linear => "linear",
            Topology::Circular => "circular",
        };
        write!(f, "{}", res)
    }
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct Source {
    pub source: String,
    pub organism: Option<String>,
}

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct Reference {
    pub description: String,
    pub authors: Option<String>,
    pub consortium: Option<String>,
    pub title: String,
    pub journal: Option<String>,
    pub pubmed: Option<String>,
    pub remark: Option<String>,
}

/// Maximum length for which the buffer holding the sequence will be
/// preallocated. This isn't a hard limit though... should it be?
#[doc(hidden)]
pub const REASONABLE_SEQ_LEN: usize = 500 * 1000 * 1000;

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, PartialEq, Clone)]
pub struct Seq {
    /// Name as specified in the LOCUS line
    pub name: Option<String>,
    /// Whether this molecule is linear or circular
    pub topology: Topology,
    /// The date specified in the LOCUS line
    pub date: Option<Date>,
    /// Length as specified in the LOCUS line. Note that this may differ from
    /// `seq.len()`, especially if `contig` is `Some` and `seq` is empty. When
    /// parsing a file, if sequence data is provided we check that this value is
    /// equal to `seq.len()`
    pub len: Option<usize>,
    // TODO: should this be an option?
    pub molecule_type: Option<String>,
    pub division: String,
    pub definition: Option<String>,
    pub accession: Option<String>,
    pub version: Option<String>,
    pub source: Option<Source>,
    pub dblink: Option<String>,
    pub keywords: Option<String>,
    pub references: Vec<Reference>,
    pub comments: Vec<String>,
    pub seq: Vec<u8>,
    pub contig: Option<Position>,
    pub features: Vec<Feature>,
}

impl Seq {
    /// Create a new, empty `Seq`
    pub fn empty() -> Seq {
        Seq {
            seq: vec![],
            contig: None,
            definition: None,
            accession: None,
            molecule_type: None,
            division: String::from("UNK"),
            dblink: None,
            keywords: None,
            source: None,
            version: None,
            comments: vec![],
            references: vec![],
            name: None,
            topology: Topology::Linear,
            date: None,
            len: None,
            features: vec![],
        }
    }

    pub fn is_circular(&self) -> bool {
        match self.topology {
            Topology::Circular => true,
            Topology::Linear => false,
        }
    }

    /// Returns the "actual" length of the sequence. Note that this may not be
    /// equal to self.seq.len(), in the following circumstances:
    /// - `self.seq` is empty and `self.contig` is not, and the corresponding
    /// file's LOCUS line specified a length. The returned value is i64 to
    /// simplifiy arithmetic with coords from `Position`
    pub fn len(&self) -> i64 {
        if let Some(len) = self.len {
            assert!(self.seq.is_empty() || len == self.seq.len());
            len as i64
        } else {
            self.seq.len() as i64
        }
    }

    /// "Normalises" an exclusive range on the chromosome, to make handling circular
    /// sequences simpler. For linear sequences, it doesn't do anything except
    /// panic unless `start` and `end` are both within the sequence, or if `end
    /// <= start`. For circular sequences, all values, including negative values,
    /// are allowed.
    ///
    /// If `end` <= `start`, the range is assumed to wrap around. The returned
    /// values will satisfy the following conditions:
    /// - `0 <= first < len`
    /// - `first < last`
    /// This means that in the case of a range which wraps around, `last` >= `len`.
    pub fn unwrap_range(&self, start: i64, end: i64) -> (i64, i64) {
        let len = self.len();
        match self.topology {
            Topology::Linear => {
                assert!(start < end);
                assert!(start >= 0 && start < len && end > 0 && end <= len);
                (start, end)
            }
            Topology::Circular => {
                let mut start = start;
                let mut end = end;

                if start >= end {
                    end += len;
                }

                while start >= len {
                    start -= len;
                    end -= len;
                }

                while start < 0 {
                    start += len;
                    end += len;
                }

                assert!(start >= 0 && start < len);
                assert!(end > start);

                (start, end)
            }
        }
    }
    /// Given a range on this sequence, returns a corresponding `Position`
    /// Note: this is a rust-style, exclusive range
    pub fn range_to_position(&self, start: i64, end: i64) -> Position {
        match self.topology {
            Topology::Linear => {
                if start + 1 == end {
                    Position::Single(start)
                } else {
                    Position::simple_span(start, end - 1)
                }
            }
            Topology::Circular => {
                let (start, end) = self.unwrap_range(start, end);
                if end > self.len() {
                    assert!(end < self.len() * 2, "Range wraps around more than once!");
                    Position::Join(vec![
                        Position::simple_span(start, self.len() - 1),
                        Position::simple_span(0, end - self.len() - 1),
                    ])
                } else if start == end {
                    Position::Single(start)
                } else {
                    Position::simple_span(start, end - 1)
                }
            }
        }
    }
    /// "Unwraps" a position on a circular sequence, so that coordinates that
    /// span the origin are replaced with positions beyond the origin.
    pub fn unwrap_position(&self, p: Position) -> Result<Position, PositionError> {
        let (first, last) = p.find_bounds()?;
        if first < 0 || last >= self.len() {
            return Err(PositionError::OutOfBounds(p));
        }
        if last < first && !self.is_circular() {
            return Err(PositionError::OutOfBounds(p));
        }
        let len = self.len();
        p.transform(&|p| Ok(p), &|v| {
            let res = if v < first { v + len } else { v };
            Ok(res)
        })
    }

    /// "Wraps" a position on a circular sequence, so that coordinates that
    /// extend beyond the end of the sequence are are wrapped to the origin.
    pub fn wrap_position(&self, p: Position) -> Result<Position, PositionError> {
        use Position::*;
        let res = p
            .transform(
                &|p| {
                    let res = match p {
                        Single(mut a) => {
                            while a >= self.len() {
                                a -= self.len();
                            }
                            Single(a)
                        }
                        Span((mut a, before), (mut b, after)) => {
                            while a >= self.len() {
                                a -= self.len();
                                b -= self.len();
                            }
                            if b < self.len() {
                                Span((a, before), (b, after))
                            } else {
                                Join(vec![
                                    Span((a, before), (self.len() - 1, After(false))),
                                    Span((0, Before(false)), (b - self.len(), after)),
                                ])
                            }
                        }
                        p => p,
                    };
                    Ok(res)
                },
                &|v| Ok(v),
            )
            .unwrap();
        simplify(res)
    }

    /// "Shifts" a feature forwards by `shift` NTs (can be negative)
    /// Note: If this fails you won't get the original `Feature`
    /// back. If this is important, you should clone first
    pub fn relocate_feature(&self, f: Feature, shift: i64) -> Result<Feature, PositionError> {
        let res = Feature {
            pos: self.relocate_position(f.pos, shift)?,
            ..f
        };
        Ok(res)
    }

    /// "Shifts" a position forwards by `shift` NTs (can be negative)
    /// Note: If this fails you won't get the original `Position`
    /// back. If this is important, you should clone first
    pub fn relocate_position(
        &self,
        p: Position,
        mut shift: i64,
    ) -> Result<Position, PositionError> {
        if self.is_circular() {
            while shift < 0 {
                shift += self.len();
            }
            while shift >= self.len() {
                shift -= self.len();
            }
            let moved = p.transform(&|p| Ok(p), &|v| Ok(v + shift)).unwrap(); // can't fail
            self.wrap_position(moved)
        } else {
            p.transform(&|p| Ok(p), &|v| Ok(v + shift))
        }
    }

    /// Used by `revcomp`
    fn revcomp_position(&self, p: Position) -> Position {
        let p = p
            .transform(
                &|mut p| {
                    match p {
                        Position::Join(ref mut positions)
                        | Position::Order(ref mut positions)
                        | Position::Bond(ref mut positions)
                        | Position::OneOf(ref mut positions) => {
                            positions.reverse();
                        }
                        _ => (),
                    };
                    let p = match p {
                        Position::Span((a, Before(before)), (b, After(after))) => {
                            Position::Span((b, Before(after)), (a, After(before)))
                        }
                        Position::Between(a, b) => Position::Between(b, a),
                        p => p,
                    };
                    Ok(p)
                },
                &|v| Ok(self.len() - 1 - v),
            )
            .unwrap(); // can't fail
        match p {
            Position::Complement(x) => *x,
            x => Position::Complement(Box::new(x)),
        }
    }

    /// Note: If this fails you won't get the original `Feature`
    /// back. If this is important, you should clone first
    pub fn revcomp_feature(&self, f: Feature) -> Feature {
        Feature {
            pos: self.revcomp_position(f.pos),
            ..f
        }
    }

    /// Returns the reverse complement of a `Seq`, skipping any features
    /// which can't be processed with a warning
    pub fn revcomp(&self) -> Seq {
        let features = self
            .features
            .iter()
            .cloned()
            .map(|f| self.revcomp_feature(f))
            .collect();
        Seq {
            features,
            seq: revcomp(&self.seq),
            ..self.clone()
        }
    }

    /// Extracts just the sequence from `start` to `end`, taking into
    /// account circularity. Note that `end` is exclusive. Use
    /// this instead of `extract_range` if you don't need the
    /// features.
    pub fn extract_range_seq(&self, start: i64, end: i64) -> Cow<[u8]> {
        // Here we use usize everywhere for convenience, since we will never
        // have to deal with negative values
        let len = self.len() as usize;
        assert!(len == self.seq.len());
        let (start, end) = self.unwrap_range(start, end);
        let mut start = start as usize;
        let mut end = end as usize;
        if end <= len {
            Cow::from(&self.seq[start..end])
        } else {
            assert!(self.is_circular());
            let mut res = Vec::with_capacity(end - start);
            while end != 0 {
                let slice_end = cmp::min(end, len);
                end -= slice_end;
                res.extend(&self.seq[start..slice_end]);
                start = 0;
            }
            Cow::from(res)
        }
    }

    /// Extracts from `start` to `end`, keeping only features which
    /// fall entirely within this range. Note that `end` is not
    /// inclusive. Skips ambiguous features with a warning.
    pub fn extract_range_no_truncation(&self, start: i64, end: i64) -> Seq {
        debug!("Extract: {} to {}, len is {}", start, end, self.len());
        let (start, end) = self.unwrap_range(start, end);
        debug!("Normalised: {} to {}", start, end);
        let shift = -start;
        let mut features = Vec::new();
        for f in &self.features {
            if let Ok((x, y)) = f.pos.find_bounds() {
                if (x < 0 || y < 0 || x > self.len() || y > self.len())
                    || (!self.is_circular() && y < x)
                {
                    warn!("Skipping feature with invalid position: {}", f.pos);
                    continue;
                }
                let (mut x, mut y) = self.unwrap_range(x, y + 1); // to exclusive
                y -= 1; // back again
                if x < start {
                    x += self.len();
                    y += self.len();
                }
                if x >= start && y < end {
                    match self.relocate_feature(f.clone(), shift) {
                        Ok(f) => features.push(f),
                        Err(e) => warn!("Skipping feature with tricky position: {}", e),
                    }
                }
            }
        }
        Seq {
            features,
            seq: self.extract_range_seq(start, end).into(),
            ..Seq::empty()
        }
    }

    /// Extracts from `start` to `end`, truncating features that
    /// extend beyond this range.  Note that `end` is not
    /// inclusive. Skips ambiguous features with a warning.
    pub fn extract_range(&self, start: i64, end: i64) -> Seq {
        let (start, end) = self.unwrap_range(start, end);
        let mut shift = -start;
        if self.is_circular() {
            while shift < 0 {
                shift += self.len();
            }
            while shift > self.len() {
                shift -= self.len();
            }
        }
        let mut features = Vec::new();
        let mut process_feature = |f: &Feature| -> Result<(), PositionError> {
            let (x, y) = f.pos.find_bounds()?;
            if (x < 0 || y < 0 || x > self.len() || y > self.len())
                || (!self.is_circular() && y < x)
            {
                return Err(PositionError::OutOfBounds(f.pos.clone()));
            }
            let relocated = self.relocate_position(f.pos.clone(), shift)?;
            if let Some(truncated) = relocated.truncate(0, end - start) {
                features.push(Feature {
                    pos: truncated,
                    ..f.clone()
                });
            }
            Ok(())
        };
        for f in &self.features {
            if let Err(e) = process_feature(f) {
                warn!("Skipping feature with tricky position: {}", e);
            }
        }
        Seq {
            features,
            seq: self.extract_range_seq(start, end).into(),
            ..Seq::empty()
        }
    }

    /// Returns a new `Seq`, rotated so that `origin` is at the start
    pub fn set_origin(&self, origin: i64) -> Seq {
        assert!(self.is_circular());
        assert!(origin < self.len());
        let rotated = self.extract_range_seq(origin, origin);
        Seq {
            seq: rotated.into(),
            features: self
                .features
                .iter()
                .cloned()
                .flat_map(|f| self.relocate_feature(f, -origin))
                .collect(),
            ..self.clone()
        }
    }

    pub fn write<T: Write>(&self, file: T) -> io::Result<()> {
        crate::writer::write(file, self)
    }
}

//TODO: should we merge adjacent positions when Before/After is set?
fn merge_adjacent(ps: Vec<Position>) -> Vec<Position> {
    use Position::*;
    let mut res: Vec<Position> = Vec::with_capacity(ps.len());
    for p in ps {
        if let Some(last) = res.last_mut() {
            match (&last, p) {
                (Single(ref a), Single(b)) => {
                    if *a + 1 == b {
                        *last = Position::simple_span(*a, b);
                    } else if *a != b {
                        // ie. join(1,1) (can this happen?)
                        res.push(Single(b));
                    }
                }
                (Single(ref a), Span((c, Before(false)), d)) => {
                    if *a + 1 == c {
                        *last = Span((*a, Before(false)), d);
                    } else {
                        res.push(Span((c, Before(false)), d));
                    }
                }
                (Span(ref a, (ref b, After(false))), Single(d)) => {
                    if *b + 1 == d {
                        *last = Span(*a, (d, After(false)));
                    } else {
                        res.push(Single(d));
                    }
                }
                (Span(a, (ref b, After(false))), Span((c, Before(false)), d)) => {
                    if *b + 1 == c {
                        *last = Span(*a, d);
                    } else {
                        res.push(Span((c, Before(false)), d));
                    }
                }
                (_, p) => res.push(p),
            }
        } else {
            res.push(p);
        }
    }
    res
}

fn flatten_join(v: Vec<Position>) -> Vec<Position> {
    let mut res = Vec::with_capacity(v.len());
    for p in v {
        match p {
            Position::Join(xs) => res.extend(flatten_join(xs)),
            p => res.push(p),
        }
    }
    res
}

/// This doesn't simplify everything yet...
/// TODO: return original Position somehow on failure
fn simplify(p: Position) -> Result<Position, PositionError> {
    p.transform(&simplify_shallow, &|v| Ok(v))
}

fn simplify_shallow(p: Position) -> Result<Position, PositionError> {
    use Position::*;
    match p {
        Join(xs) => {
            if xs.is_empty() {
                return Err(PositionError::Empty);
            }
            let xs = flatten_join(xs);
            let mut xs = merge_adjacent(xs);
            assert!(!xs.is_empty());
            if xs.len() == 1 {
                // remove the join, we now have a new type of position
                // so we need to simplify again
                Ok(simplify_shallow(xs.pop().unwrap())?)
            } else {
                Ok(Join(xs))
            }
        }
        Span((a, Before(false)), (b, After(false))) if a == b => Ok(Single(a)),
        p => Ok(p),
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::tests::init;

    #[test]
    fn test_merge_adj() {
        use Position::*;
        assert_eq!(
            merge_adjacent(vec![
                Position::simple_span(0, 3),
                Position::simple_span(5, 7),
            ]),
            vec![Position::simple_span(0, 3), Position::simple_span(5, 7)]
        );
        assert_eq!(
            merge_adjacent(vec![
                Position::simple_span(0, 4),
                Position::simple_span(5, 7),
            ]),
            vec![Position::simple_span(0, 7)]
        );
        assert_eq!(
            merge_adjacent(vec![Position::simple_span(0, 4), Position::Single(5)]),
            vec![Position::simple_span(0, 5)]
        );
        assert_eq!(
            merge_adjacent(vec![Single(0), Position::simple_span(1, 5)]),
            vec![Position::simple_span(0, 5)]
        );
        assert_eq!(
            merge_adjacent(vec![Single(0), Single(1)]),
            vec![Position::simple_span(0, 1)]
        );
        assert_eq!(
            &Join(merge_adjacent(vec![
                Position::Span((1, Before(true)), (2, After(false))),
                Position::Span((3, Before(false)), (4, After(true)))
            ]))
            .to_gb_format(),
            "join(<2..>5)"
        );
    }

    #[test]
    fn pos_relocate_circular() {
        let s = Seq {
            seq: b"0123456789".to_vec(),
            topology: Topology::Circular,
            ..Seq::empty()
        };
        assert_eq!(
            s.relocate_position(Position::Single(5), -9),
            Ok(Position::Single(6))
        );
        let span1 = Position::simple_span(7, 9);
        let span2 = Position::simple_span(0, 3);
        assert_eq!(span1.to_gb_format(), String::from("8..10"));
        let join = Position::Join(vec![span1, span2]);
        assert_eq!(
            s.relocate_position(join, -5).unwrap().to_gb_format(),
            String::from("3..9")
        );
    }

    #[test]
    fn relocate_position() {
        let s = Seq {
            seq: b"0123456789".to_vec(),
            topology: Topology::Circular,
            ..Seq::empty()
        };
        assert_eq!(
            &s.relocate_position(Position::simple_span(0, 9), 5)
                .unwrap()
                .to_gb_format(),
            "join(6..10,1..5)"
        );
        assert_eq!(
            &s.relocate_position(Position::simple_span(5, 7), 5)
                .unwrap()
                .to_gb_format(),
            "1..3"
        );
        assert_eq!(
            &s.relocate_position(
                Position::Join(vec![
                    Position::simple_span(7, 9),
                    Position::simple_span(0, 2)
                ]),
                2
            )
            .unwrap()
            .to_gb_format(),
            "join(10,1..5)"
        );
        assert_eq!(
            &s.relocate_position(
                Position::Join(vec![
                    Position::simple_span(8, 9),
                    Position::simple_span(0, 1)
                ]),
                2
            )
            .unwrap()
            .to_gb_format(),
            "1..4"
        );
        assert_eq!(
            &s.relocate_position(Position::simple_span(0, 2), 5)
                .unwrap()
                .to_gb_format(),
            "6..8"
        );
        assert_eq!(
            s.relocate_position(Position::Single(5), -9).unwrap(),
            Position::Single(6)
        );
        let span1 = Position::simple_span(7, 9);
        let span2 = Position::simple_span(0, 3);
        let join = Position::Join(vec![span1, span2]);
        assert_eq!(
            &s.relocate_position(join, -5).unwrap().to_gb_format(),
            "3..9"
        );
    }

    #[test]
    fn extract_range_seq() {
        let s = Seq {
            seq: b"0123456789".to_vec(),
            topology: Topology::Circular,
            ..Seq::empty()
        };
        assert_eq!(
            s.extract_range_seq(0, 10).into_owned(),
            b"0123456789".to_vec()
        );
        assert_eq!(
            s.extract_range_seq(0, 11).into_owned(),
            b"01234567890".to_vec()
        );
        assert_eq!(
            s.extract_range_seq(-1, 10).into_owned(),
            b"90123456789".to_vec()
        );
        assert_eq!(
            s.extract_range_seq(-1, 11).into_owned(),
            b"901234567890".to_vec()
        );
        assert_eq!(
            s.extract_range_seq(-1, 21).into_owned(),
            b"9012345678901234567890".to_vec()
        );
    }

    #[test]
    fn extract_seq_no_truncate() {
        use std::iter::repeat;
        let mut features = Vec::new();
        for i in 0..8 {
            let pos = Position::simple_span(i, i + 2);
            features.push(Feature {
                // this _is_ inclusive
                pos: pos.clone(),
                kind: FeatureKind::from(format!("{}", pos)),
                qualifiers: vec![],
            });
        }
        for i in 8..10 {
            features.push(Feature {
                pos: Position::Join(vec![
                    Position::simple_span(i, 9),
                    Position::simple_span(0, i - 8),
                ]),
                kind: FeatureKind::from(format!("{}", i)),
                qualifiers: Vec::new(),
            });
        }
        let s = Seq {
            seq: repeat(b'A').take(10).collect(),
            topology: Topology::Linear,
            features,
            ..Seq::empty()
        };
        for i in 0..8 {
            // this _isn't_ inclusive
            let reg = s.extract_range_no_truncation(i, i + 3);
            assert_eq!(reg.features.len(), 1);
        }
        let s = Seq {
            topology: Topology::Circular,
            ..s
        };
        for i in -10..20 {
            let reg = s.extract_range_no_truncation(i, i + 3);
            assert_eq!(reg.features.len(), 1);
            println!(
                "{}, original pos {}, current pos {}",
                i, reg.features[0].kind, reg.features[0].pos
            );
            assert_eq!(reg.features[0].pos, Position::simple_span(0, 2));
        }
    }

    #[test]
    fn test_extract_linear() {
        let features = vec![Feature {
            pos: Position::simple_span(0, 99),
            kind: FeatureKind::from(""),
            qualifiers: Vec::new(),
        }];
        let s = Seq {
            seq: ::std::iter::repeat(b'A').take(100).collect(),
            topology: Topology::Linear,
            features,
            ..Seq::empty()
        };
        for i in 0..91 {
            for j in 1..11 {
                let res = s.extract_range(i, i + j);
                assert_eq!(res.features.len(), 1);
                assert_eq!(
                    res.features[0].pos,
                    simplify(Position::simple_span(0, j - 1)).unwrap()
                );
            }
        }
    }

    #[test]
    fn test_extract_circular() {
        let whole_seq = Feature {
            pos: Position::simple_span(0, 9),
            kind: FeatureKind::from(""),
            qualifiers: Vec::new(),
        };
        let make_pos = |from: i64, to: i64| -> Position {
            if to >= 10 {
                Position::Join(vec![
                    simplify(Position::simple_span(from, 9)).unwrap(),
                    simplify(Position::simple_span(0, to - 10)).unwrap(),
                ])
            } else {
                simplify(Position::simple_span(from, to)).unwrap()
            }
        };

        for i in 0..10 {
            for j in 1..3 {
                let s = Seq {
                    seq: ::std::iter::repeat(b'A').take(10).collect(),
                    topology: Topology::Circular,
                    features: vec![
                        whole_seq.clone(),
                        Feature {
                            pos: make_pos(i, i + j - 1),
                            kind: FeatureKind::from(""),
                            qualifiers: Vec::new(),
                        },
                    ],
                    ..Seq::empty()
                };
                let res = s.extract_range(i, i + j);
                assert_eq!(res.features.len(), 2);
                if i < 8 {
                    assert_eq!(
                        res.features[0].pos,
                        simplify(Position::simple_span(0, j - 1)).unwrap()
                    );
                }
                println!("1 {:?}", res.features[0]);
                println!("{} {}", i, j);
                if i < 8 {
                    assert_eq!(
                        res.features[1].pos,
                        simplify(Position::simple_span(0, j - 1)).unwrap()
                    );
                }
                println!("2 {:?}", res.features[1]);
            }
        }
    }

    #[test]
    fn extract_exclude_features() {
        let s = Seq {
            topology: Topology::Circular,
            seq: (0..10).collect(),
            features: vec![Feature {
                pos: Position::simple_span(0, 3),
                kind: feature_kind!(""),
                qualifiers: vec![],
            }],
            ..Seq::empty()
        };
        let res = s.extract_range(4, 10);
        assert_eq!(res.features, vec![]);
        let res = s.extract_range(0, 1);
        assert_eq!(res.features.len(), 1);
        assert_eq!(&res.features[0].pos, &Position::Single(0));
        let res = s.extract_range(3, 4);
        assert_eq!(res.features.len(), 1);
        assert_eq!(&res.features[0].pos, &Position::Single(0));
        let res = s.extract_range(0, 10);
        assert_eq!(&res.features[0].pos, &Position::simple_span(0, 3));
    }

    #[test]
    fn truncate() {
        assert_eq!(
            Position::Single(0).truncate(0, 1),
            Some(Position::Single(0))
        );
        assert_eq!(Position::Single(0).truncate(1, 2), None);
        assert_eq!(
            Position::simple_span(0, 2).truncate(1, 2),
            Some(Position::Single(1))
        );
        assert_eq!(
            Position::simple_span(0, 1).truncate(0, 2),
            Some(Position::simple_span(0, 1))
        );
        assert_eq!(
            Position::Complement(Box::new(Position::simple_span(0, 1))).truncate(0, 2),
            Some(Position::Complement(Box::new(Position::simple_span(0, 1))))
        );
        assert_eq!(
            Position::Complement(Box::new(Position::simple_span(0, 1))).truncate(10, 20),
            None
        );
        assert_eq!(Position::simple_span(0, 1).truncate(3, 4), None);
        let p = Position::Join(vec![
            Position::simple_span(0, 2),
            Position::simple_span(4, 6),
        ]);
        assert_eq!(p.truncate(0, 3), Some(Position::simple_span(0, 2)));
        assert_eq!(p.truncate(10, 30), None);
    }

    #[test]
    fn test_extract_circular_split() {
        let features = vec![Feature {
            pos: Position::Join(vec![
                Position::simple_span(0, 2),
                Position::simple_span(4, 9),
            ]),
            kind: FeatureKind::from(""),
            qualifiers: Vec::new(),
        }];
        let s = Seq {
            seq: (0..10).collect(),
            topology: Topology::Circular,
            features,
            ..Seq::empty()
        };

        for i in 0..10 {
            let res = s.extract_range(i, i + 10);
            println!("{:?}", res.features);
        }
    }

    #[test]
    fn set_origin() {
        init();
        let seq = Seq {
            seq: "0123456789".into(),
            features: vec![
                Feature {
                    pos: Position::simple_span(2, 6),
                    kind: feature_kind!(""),
                    qualifiers: Vec::new(),
                },
                Feature {
                    pos: Position::simple_span(0, 9),
                    kind: feature_kind!(""),
                    qualifiers: Vec::new(),
                },
                Feature {
                    pos: Position::Join(vec![
                        Position::simple_span(7, 9),
                        Position::simple_span(0, 3),
                    ]),
                    kind: feature_kind!(""),
                    qualifiers: Vec::new(),
                },
                Feature {
                    pos: Position::Single(0),
                    kind: feature_kind!(""),
                    qualifiers: Vec::new(),
                },
            ],
            topology: Topology::Circular,
            ..Seq::empty()
        };
        for i in 1..9 {
            println!("******** {}", i);
            let rotated = seq.set_origin(i);
            println!("rotated {:?}", rotated.features);
            let rotated2 = rotated.set_origin(10 - i);
            println!("rotated2: {:?}", rotated2.features);
            assert_eq!(rotated2.features, seq.features);
        }
    }
    #[test]
    fn range_to_position() {
        let s = Seq {
            seq: "0123456789".into(),
            topology: Topology::Linear,
            ..Seq::empty()
        };
        assert_eq!(s.range_to_position(0, 10), Position::simple_span(0, 9));
        let s = Seq {
            topology: Topology::Circular,
            ..s
        };
        assert_eq!(s.range_to_position(5, 10), Position::simple_span(5, 9));
        assert_eq!(
            s.range_to_position(5, 11).to_gb_format(),
            "join(6..10,1..1)"
        );
        assert_eq!(
            s.range_to_position(5, 15).to_gb_format(),
            "join(6..10,1..5)"
        );
    }
    #[test]
    fn unwrap_range_linear() {
        let s = Seq {
            seq: "01".into(),
            ..Seq::empty()
        };
        assert_eq!(s.unwrap_range(0, 1), (0, 1));
        assert_eq!(s.unwrap_range(0, 2), (0, 2));
    }

    #[test]
    #[should_panic]
    fn unwrap_range_linear_bad() {
        let s = Seq {
            seq: "01".into(),
            ..Seq::empty()
        };
        let _ = s.unwrap_range(0, 3);
    }

    #[test]
    fn unwrap_range_circular() {
        let s = Seq {
            seq: "01".into(),
            topology: Topology::Circular,
            ..Seq::empty()
        };
        assert_eq!(s.unwrap_range(0, 1), (0, 1));
        assert_eq!(s.unwrap_range(0, 2), (0, 2));
        assert_eq!(s.unwrap_range(0, 3), (0, 3));
        assert_eq!(s.unwrap_range(-1, 0), (1, 2));
        assert_eq!(s.unwrap_range(1, 2), (1, 2));
        assert_eq!(s.unwrap_range(2, 3), (0, 1));
        assert_eq!(s.unwrap_range(-2, -1), (0, 1));
    }

    #[test]
    fn unwrap_position() {
        let mut s = Seq {
            seq: "0123456789".into(),
            topology: Topology::Linear,
            features: Vec::new(),
            ..Seq::empty()
        };
        let pos = Position::simple_span(2, 4);
        assert_eq!(s.unwrap_position(pos).unwrap(), Position::simple_span(2, 4));
        s.topology = Topology::Circular;
        let pos = Position::simple_span(7, 3);
        assert_eq!(
            s.unwrap_position(pos).unwrap(),
            Position::simple_span(7, 13)
        )
    }

    #[test]
    fn wrap_position() {
        let s = Seq {
            seq: (0..10).collect(),
            topology: Topology::Circular,
            ..Seq::empty()
        };
        assert_eq!(
            Position::Single(0),
            s.wrap_position(Position::Single(0)).unwrap()
        );
        assert_eq!(
            Position::Single(0),
            s.wrap_position(Position::Single(10)).unwrap()
        );
        assert_eq!(
            Position::Single(1),
            s.wrap_position(Position::Single(11)).unwrap()
        );
        assert_eq!(
            Position::simple_span(0, 1),
            s.wrap_position(Position::simple_span(10, 11)).unwrap()
        );
        assert_eq!(
            Position::Join(vec![Position::Single(9), Position::simple_span(0, 4)]),
            s.wrap_position(Position::simple_span(9, 14)).unwrap()
        );
        assert_eq!(
            &s.wrap_position(Position::Span((8, Before(false)), (10, After(true))))
                .unwrap()
                .to_gb_format(),
            "join(9..10,1..>1)"
        );
    }

    #[test]
    fn revcomp() {
        let make_seq = |positions: Vec<Position>| Seq {
            seq: "aaaaaaaaat".into(),
            topology: Topology::Linear,
            features: positions
                .into_iter()
                .map(|p| Feature {
                    pos: p,
                    kind: feature_kind!(""),
                    qualifiers: Vec::new(),
                })
                .collect(),
            ..Seq::empty()
        };
        assert_eq!(
            make_seq(vec![Position::simple_span(0, 1)])
                .revcomp()
                .features[0]
                .pos,
            Position::Complement(Box::new(Position::simple_span(8, 9)))
        );
        assert_eq!(
            make_seq(vec![Position::Join(vec![
                Position::simple_span(0, 1),
                Position::simple_span(3, 4)
            ])])
            .revcomp()
            .features[0]
                .pos,
            Position::Complement(Box::new(Position::Join(vec![
                Position::simple_span(5, 6),
                Position::simple_span(8, 9)
            ])))
        );
        assert_eq!(
            make_seq(vec![Position::Single(9)]).revcomp().features[0].pos,
            Position::Complement(Box::new(Position::Single(0)))
        );
    }
}
