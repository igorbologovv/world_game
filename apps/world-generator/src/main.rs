use std::{error::Error, path::Path};

fn main() -> Result<(), Box<dyn Error>> {
    world_generation::generate_world(Path::new("assets"))
}
