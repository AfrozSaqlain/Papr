use ratatui::widgets::{ListState, List, ListItem};
use ratatui::layout::Rect;
use ratatui::buffer::Buffer;
use ratatui::widgets::StatefulWidget;

fn main() {
    let mut offset = 0;
    
    // Simulate a list of 10 items, viewport height 5.
    let items = vec![
        ListItem::new("1"), ListItem::new("2"), ListItem::new("3"), ListItem::new("4"), ListItem::new("5"),
        ListItem::new("6"), ListItem::new("7"), ListItem::new("8"), ListItem::new("9"), ListItem::new("10"),
    ];
    let list = List::new(items);
    let area = Rect::new(0, 0, 10, 5);
    let mut buf = Buffer::empty(area.clone());

    // Scroll down to item 7 (index 6). Viewport should be items [3..7] (index 2..6), so offset = 2
    let mut state = ListState::default().with_selected(Some(6)).with_offset(0);
    StatefulWidget::render(list.clone(), area.clone(), &mut buf, &mut state);
    offset = state.offset();
    println!("Selected: {}, Offset: {}", state.selected().unwrap(), offset); // Expected: 2

    // Now move selection UP to item 6 (index 5). Viewport should remain [3..7] (offset = 2)
    let mut state2 = ListState::default().with_selected(Some(5)).with_offset(offset);
    StatefulWidget::render(list.clone(), area.clone(), &mut buf, &mut state2);
    offset = state2.offset();
    println!("Selected: {}, Offset: {}", state2.selected().unwrap(), offset); // Expected: 2
}
