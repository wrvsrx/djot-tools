use comfy_table::{presets::NOTHING, ContentArrangement, Table};

use crate::task_ops::TaskOutputRecord;

pub(crate) fn task_table(records: &[TaskOutputRecord], heading: bool) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);
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

        let with_heading = task_table(&records, true);
        assert!(with_heading.contains("S"));
        assert!(with_heading.contains("Task"));
        assert!(with_heading.contains("Source"));
        assert!(with_heading.contains("Write parser"));

        let without_heading = task_table(&records, false);
        assert!(!without_heading.contains("Task"));
        assert!(without_heading.contains("Write parser"));
    }
}
