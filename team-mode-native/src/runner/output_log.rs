use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;

pub fn append_line(path: impl AsRef<Path>, line: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(line.as_bytes())?;
    if !line.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    file.flush()
}

pub fn read_last_lines(path: impl AsRef<Path>, max_lines: usize) -> io::Result<Vec<String>> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    for line in reader.lines() {
        lines.push(line?);
        if lines.len() > max_lines {
            lines.remove(0);
        }
    }
    Ok(lines)
}
