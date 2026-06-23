use comfy_table::{presets::NOTHING, ContentArrangement, Table};

use crate::task_ops::TaskOutputRecord;

pub(crate) fn task_table(records: &[TaskOutputRecord]) -> String {
    let mut table = Table::new();
    table
        .load_preset(NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);
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
