//! Which fabricator's table a board is checked against. One answer, read by
//! the command line, the language server and the engine the browser runs.
//!
//! The command line picked the table for the board's layer count and the two
//! editors did not: a four-layer board naming no fab was checked against
//! `jlcpcb_standard_4layer` by `cypcb check` and against
//! `jlcpcb_standard_2layer` in both editors, so a 0.11mm gap was clean in one
//! and an error in the other. Each surface had its own copy of the choice.
//! This is the only one left.
//!
//! The order: a name the caller gives (the `--preset` flag), else the
//! board's own `fab`, else JLCPCB. A house name - `jlcpcb`, `oshpark` -
//! follows the board's copper layer count; a table name, which holds
//! `layer`, is taken as written.

use cypcb_rules::presets::RulesPreset;
use cypcb_world::BoardWorld;

use crate::net_rules::preset_for_world;

/// Where a table name was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Given by the caller, such as the command line's `--preset`.
    Caller,
    /// The board's own `fab` line.
    Design,
}

/// A name that is not a table this tool has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTable {
    /// The name as written.
    pub name: String,
    /// Where it was written.
    pub origin: Origin,
    /// The table an editor checks against instead, since it has to go on
    /// drawing the board: JLCPCB, for the board's layer count.
    pub fallback: RulesPreset,
}

/// The table `world` is checked against, given the name the caller asked
/// for, if any.
pub fn table_for(asked: Option<&str>, world: &BoardWorld) -> Result<RulesPreset, UnknownTable> {
    let written = match (asked, world.fab()) {
        (Some(name), _) => Some((name, Origin::Caller)),
        (None, Some(name)) => Some((name, Origin::Design)),
        (None, None) => None,
    };
    let default = preset_for_world(RulesPreset::JlcpcbStandard2Layer, world);
    let Some((name, origin)) = written else {
        return Ok(default);
    };
    let Some(chosen) = RulesPreset::from_name(name) else {
        return Err(UnknownTable {
            name: name.to_string(),
            origin,
            fallback: default,
        });
    };
    if name.to_ascii_lowercase().contains("layer") {
        return Ok(chosen);
    }
    Ok(preset_for_world(chosen, world))
}

/// The table an editor checks `world` against: [`table_for`] with nothing
/// asked, and the fallback when the board names a fab this tool does not
/// have. The unknown name comes back too, so the editor can say so.
pub fn table_for_editor(world: &BoardWorld) -> (RulesPreset, Option<UnknownTable>) {
    match table_for(None, world) {
        Ok(table) => (table, None),
        Err(unknown) => (unknown.fallback, Some(unknown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cypcb_world::components::Fab;

    fn board(layers: u8, fab: Option<&str>) -> BoardWorld {
        let mut world = BoardWorld::new();
        world.set_board(
            "b".to_string(),
            (cypcb_core::Nm::from_mm(20.0), cypcb_core::Nm::from_mm(20.0)),
            layers,
        );
        if let Some(fab) = fab {
            world.set_fab(Fab(fab.to_string()));
        }
        world
    }

    #[test]
    fn the_design_is_read_when_nothing_is_asked() {
        assert_eq!(
            table_for(None, &board(2, Some("oshpark"))),
            Ok(RulesPreset::OshPark2Layer)
        );
    }

    #[test]
    fn what_is_asked_wins_over_the_design() {
        assert_eq!(
            table_for(Some("pcbway"), &board(2, Some("oshpark"))),
            Ok(RulesPreset::from_name("pcbway").unwrap())
        );
    }

    #[test]
    fn saying_nothing_is_jlcpcb_for_the_layer_count() {
        assert_eq!(
            table_for(None, &board(2, None)),
            Ok(RulesPreset::JlcpcbStandard2Layer)
        );
        assert_eq!(
            table_for(None, &board(4, None)),
            Ok(RulesPreset::JlcpcbStandard4Layer)
        );
    }

    #[test]
    fn a_house_follows_the_layer_count_and_a_table_does_not() {
        assert_eq!(
            table_for(None, &board(4, Some("jlcpcb"))),
            Ok(RulesPreset::JlcpcbStandard4Layer)
        );
        assert_eq!(
            table_for(Some("jlcpcb_standard_2layer"), &board(4, None)),
            Ok(RulesPreset::JlcpcbStandard2Layer)
        );
    }

    #[test]
    fn an_unknown_name_says_where_it_was_written_and_what_stands_in() {
        let from_design = table_for(None, &board(4, Some("jlpcb"))).unwrap_err();
        assert_eq!(from_design.origin, Origin::Design);
        assert_eq!(from_design.name, "jlpcb");
        assert_eq!(from_design.fallback, RulesPreset::JlcpcbStandard4Layer);

        let from_caller = table_for(Some("jlpcb"), &board(2, None)).unwrap_err();
        assert_eq!(from_caller.origin, Origin::Caller);
    }
}
