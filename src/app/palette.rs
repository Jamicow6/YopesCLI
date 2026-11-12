use super::*;

impl App {
    pub(crate) fn palette_matches(&self) -> Vec<&'static CommandSpec> {
        if self.palette_hidden {
            return Vec::new();
        }
        matching_commands(&self.input.value)
    }

    pub(crate) fn palette_selected(&self) -> usize {
        let count = self.palette_matches().len();
        if count == 0 {
            0
        } else {
            self.palette_index.min(count - 1)
        }
    }

    pub(crate) fn palette_open(&self) -> bool {
        !self.palette_matches().is_empty()
    }

    pub(crate) fn reset_palette(&mut self) {
        self.palette_index = 0;
        self.palette_hidden = false;
    }

    pub(crate) fn palette_move(&mut self, forward: bool) {
        let count = self.palette_matches().len();
        if count == 0 {
            return;
        }
        let current = self.palette_index.min(count - 1);
        self.palette_index = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
    }

    pub(crate) fn palette_complete(&mut self) -> Option<CommandSpec> {
        let matches = self.palette_matches();
        let index = self.palette_index.min(matches.len().saturating_sub(1));
        let command = *matches.get(index).copied()?;
        self.input.set(command.completion());
        self.palette_index = 0;
        self.palette_hidden = true;
        Some(command)
    }

    pub(crate) fn palette_accept(&mut self) -> Action {
        if self.palette_complete().is_some() {
            self.submit()
        } else {
            Action::None
        }
    }
}
