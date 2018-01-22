use std::{fmt, usize};

// Compute count, average, min, max and possibly standard deviation of a vec of usize values
pub struct Stats {
    count: usize,
    average: f64,
    min: usize,
    max: usize,
    standard_deviation: Option<f64>,
}
impl Stats {
    pub fn new(values: &Vec<usize>) -> Self {
        let mut sum = 0usize;
        let mut max = 0usize;
        let mut min = usize::MAX;
        for val in values {
            sum += *val;
            if max < *val {
                max = *val;
            }
            if min > *val {
                min = *val;
            }
        }
        let count = values.len() as f64;
        let average = sum as f64 / count;
        let standard_deviation = if count == 1f64 {
            // More than one samples are needed to apply the sample standard deviation formula
            None
        } else {
            let mut variance = 0f64;
            for val in values {
                variance += (*val as f64 - average).powi(2);
            }
            variance = variance / (count - 1f64);
            let standard_deviation = variance.sqrt();
            Some(standard_deviation)
        };
        Stats {
            count: values.len(),
            average: average,
            min: min,
            max: max,
            standard_deviation: standard_deviation,
        }
    }
    pub fn get_header_line() -> &'static str {
        return &"| Count | Average | Min | Max | Standard dev |";
    }
    pub fn get_separator_line() -> &'static str {
        return &"|------:|--------:|----:|----:|-------------:|";
    }
}

// Display stats as a markdown table
impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let precision = f.precision().unwrap_or(2usize);
        try!(write!(
            f,
            "{} | {:.*} | {} | {} | ",
            self.count,
            precision,
            self.average,
            self.min,
            self.max
        ));
        match self.standard_deviation {
            None => write!(f, "None |"),
            Some(standard_deviation) => write!(f, "{:.*} |", precision, standard_deviation),
        }
    }
}
