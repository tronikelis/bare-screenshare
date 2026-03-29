use gstreamer::{self as gst, prelude::*};

pub struct Pipeline {
    elements: Vec<*const ()>,
    pipeline: gst::Pipeline,
}

impl Pipeline {
    pub fn new(pipeline: gst::Pipeline) -> Self {
        Self {
            pipeline,
            elements: vec![],
        }
    }

    fn rm_known_elements<'a>(
        &mut self,
        mut elements: Vec<&'a gst::Element>,
    ) -> Vec<&'a gst::Element> {
        let mut i: isize = -1;
        while i + 1 < elements.len() as _ {
            i += 1;

            let element_raw = elements[i as usize] as *const _ as *const ();
            if self.elements.contains(&element_raw) {
                elements.swap_remove(i as _);
                i -= 1;
            } else {
                self.elements.push(element_raw)
            }
        }
        elements
    }

    pub fn link<'a>(mut self, elements: impl IntoIterator<Item = &'a gst::Element>) -> Self {
        let elements = elements.into_iter().collect::<Vec<_>>();

        let unknown_elements = self.rm_known_elements(elements.clone());
        self.pipeline.add_many(unknown_elements).unwrap();

        gst::Element::link_many(&elements).unwrap();
        self
    }

    pub fn gst_pipeline(self) -> gst::Pipeline {
        self.pipeline
    }
}
