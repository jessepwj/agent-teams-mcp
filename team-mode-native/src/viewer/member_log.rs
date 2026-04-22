use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::Path;

pub fn tail_lines<I, S>(lines: I, max_lines: usize) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut tail = VecDeque::with_capacity(max_lines.saturating_add(1));
    for line in lines {
        if max_lines == 0 {
            continue;
        }
        tail.push_back(line.into());
        while tail.len() > max_lines {
            tail.pop_front();
        }
    }
    tail.into_iter().collect()
}

pub fn read_tail_lines(path: impl AsRef<Path>, max_lines: usize) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    for line in reader.lines() {
        lines.push(line?);
    }
    Ok(tail_lines(lines, max_lines))
}
