use super::*;

#[derive(Deserialize)]
struct TodoItemArgs {
    text: String,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct TodoWriteArgs {
    items: Vec<TodoItemArgs>,
}

const MAX_TODO_ITEMS: usize = 100;
const MAX_TODO_TEXT: usize = 500;

/// Validate the checklist and return it as JSON: the app keeps the tasks in
/// the session, so the list never needs a workspace file.
///
/// The model owns the list content: it sends the whole list every time, so
/// the stored checklist is always what the model believes. An empty list is
/// a deliberate close of the finished checklist, not a spill.
pub(super) fn todo_write(arguments: &str) -> Result<String> {
    let args: TodoWriteArgs = parse(arguments)?;
    if args.items.len() > MAX_TODO_ITEMS {
        bail!("too many items; the checklist is capped at {MAX_TODO_ITEMS}");
    }
    let items = args
        .items
        .iter()
        .map(|item| {
            let text = clip(item.text.trim(), MAX_TODO_TEXT);
            if text.is_empty() {
                bail!("item text cannot be empty");
            }
            Ok(json!({ "text": text, "done": item.done }))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!(items).to_string())
}
