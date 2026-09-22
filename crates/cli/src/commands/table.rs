use unicode_width::UnicodeWidthStr;

pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let widths = (0..headers.len())
        .map(|index| {
            rows.iter()
                .filter_map(|row| row.get(index))
                .map(|value| display_width(value))
                .chain(std::iter::once(display_width(headers[index])))
                .max()
                .unwrap_or_default()
        })
        .collect::<Vec<_>>();
    print_border('┌', '┬', '┐', &widths);
    print_row(headers.iter().copied(), &widths);
    print_border('├', '┼', '┤', &widths);
    for row in rows {
        print_row(row.iter().map(String::as_str), &widths);
    }
    print_border('└', '┴', '┘', &widths);
}

fn print_border(left: char, middle: char, right: char, widths: &[usize]) {
    print!("{left}");
    for (index, width) in widths.iter().enumerate() {
        if index > 0 {
            print!("{middle}");
        }
        print!("{}", "─".repeat(width + 2));
    }
    println!("{right}");
}

fn print_row<'a>(values: impl Iterator<Item = &'a str>, widths: &[usize]) {
    print!("│");
    for (index, value) in values.enumerate() {
        let value = normalize(value);
        print!(" {value}");
        print!("{} │", " ".repeat(widths[index] - display_width(&value)));
    }
    println!();
}

fn normalize(value: &str) -> String {
    value.replace(['\t', '\n', '\r'], " ")
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(normalize(value).as_str())
}

#[cfg(test)]
mod tests {
    use super::display_width;

    #[test]
    fn measures_wide_characters_and_normalizes_control_characters() {
        assert_eq!(display_width("客厅\tA"), 6);
    }
}
