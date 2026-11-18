//! sage-avatar — o slime na área de trabalho (viewport egui transparente, sempre no topo).
//!
//! API para o app: [`PetView::new`], [`PetView::on_event`] e [`PetView::show`].

pub mod anim;
pub mod brain;
pub mod sprites;
pub mod state;
mod view;

pub use view::PetView;
