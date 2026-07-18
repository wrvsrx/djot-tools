use comfy_table::{presets::NOTHING, ContentArrangement, Table};

use crate::task_ops::TaskOutputRecord;

pub(crate) fn task_table(records: &[TaskOutputRecord], heading: bool) -> String {
    task_table_with_width(records, heading, None)
}

fn task_table_with_width(
    records: &[TaskOutputRecord],
    heading: bool,
    width: Option<u16>,
) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);
    if let Some(width) = width {
        table.set_width(width);
    }
    if heading {
        table.set_header(["S", "Task", "Source"]);
    }
    for record in records {
        table.add_row([&record.status, &record.title, &record.source]);
    }
    table.to_string()
}

pub(crate) fn print_paths(paths: impl IntoIterator<Item = String>) {
    for path in paths {
        println!("{path}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_table_can_render_heading() {
        let records = [TaskOutputRecord {
            status: "-".to_string(),
            title: "Write parser".to_string(),
            source: "tasks.dj#write-parser".to_string(),
        }];

        let with_heading = task_table_with_width(&records, true, Some(100));
        assert!(with_heading.contains("S"));
        assert!(with_heading.contains("Task"));
        assert!(with_heading.contains("Source"));
        assert!(with_heading.contains("Write parser"));

        let without_heading = task_table_with_width(&records, false, Some(100));
        assert!(!without_heading.contains("Task"));
        assert!(without_heading.contains("Write parser"));
    }

    #[test]
    fn task_table_wraps_to_requested_width() {
        let records = [TaskOutputRecord {
            status: "-".to_string(),
            title: "Write a parser that handles narrow terminals".to_string(),
            source: "tasks.dj#write-parser".to_string(),
        }];

        let rendered = task_table_with_width(&records, true, Some(40));

        assert!(rendered.lines().all(|line| line.len() <= 40));
        assert!(rendered.lines().count() > 2);
    }
}
