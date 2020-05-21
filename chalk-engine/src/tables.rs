use crate::context::Context;
use crate::table::Table;
use crate::TableIndex;
use rustc_hash::FxHashMap;
use std::ops::{Index, IndexMut};

/// See `Forest`.
pub(crate) struct Tables<C: Context> {
    /// Maps from a canonical goal to the index of its table.
    table_indices: FxHashMap<C::UCanonicalGoalInEnvironment, TableIndex>,

    /// Table: as described above, stores the key information for each
    /// tree in the forest.
    tables: Vec<Table<C>>,
}

impl<C: Context> Tables<C> {
    pub(crate) fn new() -> Tables<C> {
        Tables {
            table_indices: FxHashMap::default(),
            tables: Vec::default(),
        }
    }

    /// The index that will be given to the next table to be inserted.
    pub(super) fn next_index(&self) -> TableIndex {
        TableIndex {
            value: self.tables.len(),
        }
    }

    pub(super) fn insert(&mut self, table: Table<C>) -> TableIndex {
        let goal = table.table_goal.clone();
        let index = self.next_index();
        self.tables.push(table);
        self.table_indices.insert(goal, index);
        index
    }

    pub(super) fn index_of(&self, literal: &C::UCanonicalGoalInEnvironment) -> Option<TableIndex> {
        self.table_indices.get(literal).cloned()
    }
}

impl<C: Context> Index<TableIndex> for Tables<C> {
    type Output = Table<C>;

    fn index(&self, index: TableIndex) -> &Table<C> {
        &self.tables[index.value]
    }
}

impl<C: Context> IndexMut<TableIndex> for Tables<C> {
    fn index_mut(&mut self, index: TableIndex) -> &mut Table<C> {
        &mut self.tables[index.value]
    }
}

impl<'a, C: Context> IntoIterator for &'a mut Tables<C> {
    type IntoIter = <&'a mut Vec<Table<C>> as IntoIterator>::IntoIter;
    type Item = <&'a mut Vec<Table<C>> as IntoIterator>::Item;

    fn into_iter(self) -> Self::IntoIter {
        IntoIterator::into_iter(&mut self.tables)
    }
}
