use anyhow::Result;

use crate::{cli::BuildArgs, indexer};

pub fn handle(args: BuildArgs) -> Result<()> {
    indexer::build::run(args)
}
