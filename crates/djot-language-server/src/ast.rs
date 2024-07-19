use core::ops::Range;
use jotdown::*;

#[derive(Debug, Clone)]
pub enum Node {
    Unified(
        super::adapter::Container,
        super::adapter::Attributes,
        Vec<Node>,
        Range<usize>,
    ),
    Leaf(super::adapter::Event, Range<usize>),
}

impl Node {
    pub fn new<'a>(
        first_event: &(Event<'a>, Range<usize>),
        events: &mut jotdown::OffsetIter<'a>,
    ) -> Self {
        let (event, range) = first_event;
        match event {
            Event::Start(c, a) => {
                let mut nodes = <Vec<Node>>::new();
                let left = range.start;
                let mut right = <Option<usize>>::None;
                while let Some((e, r)) = events.next() {
                    match &e {
                        Event::End(c_) => {
                            assert_eq!(c, c_);
                            right = Some(r.end);
                            break;
                        }
                        _ => {
                            let node = Node::new(&(e, r), events);
                            nodes.push(node);
                        }
                    }
                }
                Node::Unified(
                    super::adapter::Container::from(c),
                    super::adapter::Attributes::from(a),
                    nodes,
                    left..right.unwrap(),
                )
            }
            Event::End(_) => {
                unreachable!()
            }
            _ => Node::Leaf(super::adapter::Event::from(event), range.clone()),
        }
    }
}
