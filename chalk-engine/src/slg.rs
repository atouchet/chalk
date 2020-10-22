use crate::normalize_deep::DeepNormalizer;
use crate::{ExClause, Literal};

use chalk_derive::HasInterner;
use chalk_ir::cast::Caster;
use chalk_ir::interner::Interner;
use chalk_ir::*;
use chalk_solve::infer::ucanonicalize::UCanonicalized;
use chalk_solve::infer::unify::UnificationResult;
use chalk_solve::infer::InferenceTable;
use chalk_solve::solve::truncate;
use chalk_solve::RustIrDatabase;

use std::fmt::Debug;
use std::marker::PhantomData;

pub(crate) mod aggregate;
mod resolvent;

#[derive(Clone, Debug, HasInterner)]
pub(crate) struct SlgContext<I: Interner> {
    phantom: PhantomData<I>,
}

impl<I: Interner> SlgContext<I> {
    pub(crate) fn next_subgoal_index(ex_clause: &ExClause<I>) -> usize {
        // For now, we always pick the last subgoal in the
        // list.
        //
        // FIXME(rust-lang-nursery/chalk#80) -- we should be more
        // selective. For example, we don't want to pick a
        // negative literal that will flounder, and we don't want
        // to pick things like `?T: Sized` if we can help it.
        ex_clause.subgoals.len() - 1
    }
}
#[derive(Clone, Debug)]
pub(crate) struct SlgContextOps<'me, I: Interner> {
    program: &'me dyn RustIrDatabase<I>,
    max_size: usize,
    expected_answers: Option<usize>,
}

impl<I: Interner> SlgContextOps<'_, I> {
    pub(crate) fn new(
        program: &dyn RustIrDatabase<I>,
        max_size: usize,
        expected_answers: Option<usize>,
    ) -> SlgContextOps<'_, I> {
        SlgContextOps {
            program,
            max_size,
            expected_answers,
        }
    }

    fn identity_constrained_subst(
        &self,
        goal: &UCanonical<InEnvironment<Goal<I>>>,
    ) -> Canonical<ConstrainedSubst<I>> {
        let (mut infer, subst, _) = InferenceTable::from_canonical(
            self.program.interner(),
            goal.universes,
            &goal.canonical,
        );
        infer
            .canonicalize(
                self.program.interner(),
                &ConstrainedSubst {
                    subst,
                    constraints: Constraints::empty(self.program.interner()),
                },
            )
            .quantified
    }

    pub(crate) fn program(&self) -> &dyn RustIrDatabase<I> {
        self.program
    }

    pub(crate) fn max_size(&self) -> usize {
        self.max_size
    }
}

/// "Truncation" (called "abstraction" in the papers referenced below)
/// refers to the act of modifying a goal or answer that has become
/// too large in order to guarantee termination.
///
/// Currently we don't perform truncation (but it might me readded later).
///
/// Citations:
///
/// - Terminating Evaluation of Logic Programs with Finite Three-Valued Models
///   - Riguzzi and Swift; ACM Transactions on Computational Logic 2013
/// - Radial Restraint
///   - Grosof and Swift; 2013
pub trait TruncateOps<I: Interner> {
    /// Check if `subgoal` is too large
    fn goal_needs_truncation(&mut self, interner: &I, subgoal: &InEnvironment<Goal<I>>) -> bool;

    /// Check if `subst` is too large
    fn answer_needs_truncation(&mut self, interner: &I, subst: &Substitution<I>) -> bool;
}

pub trait ResolventOps<I: Interner> {
    /// Combines the `goal` (instantiated within `infer`) with the
    /// given program clause to yield the start of a new strand (a
    /// canonical ex-clause).
    ///
    /// The bindings in `infer` are unaffected by this operation.
    fn resolvent_clause(
        &mut self,
        interner: &I,
        environment: &Environment<I>,
        goal: &DomainGoal<I>,
        subst: &Substitution<I>,
        clause: &ProgramClause<I>,
    ) -> Fallible<ExClause<I>>;

    fn apply_answer_subst(
        &mut self,
        interner: &I,
        ex_clause: &mut ExClause<I>,
        selected_goal: &InEnvironment<Goal<I>>,
        answer_table_goal: &Canonical<InEnvironment<Goal<I>>>,
        canonical_answer_subst: &Canonical<AnswerSubst<I>>,
    ) -> Fallible<()>;
}

/// Methods for unifying and manipulating terms and binders.
pub trait UnificationOps<I: Interner> {
    // Used by: simplify
    fn instantiate_binders_universally(&mut self, interner: &I, arg: &Binders<Goal<I>>) -> Goal<I>;

    // Used by: simplify
    fn instantiate_binders_existentially(
        &mut self,
        interner: &I,
        arg: &Binders<Goal<I>>,
    ) -> Goal<I>;

    // Used by: logic (but for debugging only)
    fn debug_ex_clause<'v>(&mut self, interner: &I, value: &'v ExClause<I>) -> Box<dyn Debug + 'v>;

    // Used by: logic
    fn fully_canonicalize_goal(
        &mut self,
        interner: &I,
        value: &InEnvironment<Goal<I>>,
    ) -> (UCanonical<InEnvironment<Goal<I>>>, UniverseMap);

    // Used by: logic
    fn canonicalize_ex_clause(
        &mut self,
        interner: &I,
        value: &ExClause<I>,
    ) -> Canonical<ExClause<I>>;

    // Used by: logic
    fn canonicalize_constrained_subst(
        &mut self,
        interner: &I,
        subst: Substitution<I>,
        constraints: Vec<InEnvironment<Constraint<I>>>,
    ) -> Canonical<ConstrainedSubst<I>>;

    // Used by: logic
    fn canonicalize_answer_subst(
        &mut self,
        interner: &I,
        subst: Substitution<I>,
        constraints: Vec<InEnvironment<Constraint<I>>>,
        delayed_subgoals: Vec<InEnvironment<Goal<I>>>,
    ) -> Canonical<AnswerSubst<I>>;

    // Used by: logic
    fn invert_goal(
        &mut self,
        interner: &I,
        value: &InEnvironment<Goal<I>>,
    ) -> Option<InEnvironment<Goal<I>>>;

    /// First unify the parameters, then add the residual subgoals
    /// as new subgoals of the ex-clause.
    /// Also add region constraints.
    ///
    /// If the parameters fail to unify, then `Error` is returned
    // Used by: simplify
    fn unify_generic_args_into_ex_clause(
        &mut self,
        interner: &I,
        environment: &Environment<I>,
        a: &GenericArg<I>,
        b: &GenericArg<I>,
        ex_clause: &mut ExClause<I>,
    ) -> Fallible<()>;
}

#[derive(Clone)]
pub struct TruncatingInferenceTable<I: Interner> {
    max_size: usize,
    infer: InferenceTable<I>,
}

impl<I: Interner> TruncatingInferenceTable<I> {
    pub(crate) fn new(max_size: usize, infer: InferenceTable<I>) -> Self {
        Self { max_size, infer }
    }
}

impl<I: Interner> TruncateOps<I> for TruncatingInferenceTable<I> {
    fn goal_needs_truncation(&mut self, interner: &I, subgoal: &InEnvironment<Goal<I>>) -> bool {
        truncate::needs_truncation(interner, &mut self.infer, self.max_size, &subgoal)
    }

    fn answer_needs_truncation(&mut self, interner: &I, subst: &Substitution<I>) -> bool {
        truncate::needs_truncation(interner, &mut self.infer, self.max_size, subst)
    }
}

impl<I: Interner> UnificationOps<I> for TruncatingInferenceTable<I> {
    fn instantiate_binders_universally(&mut self, interner: &I, arg: &Binders<Goal<I>>) -> Goal<I> {
        self.infer.instantiate_binders_universally(interner, arg)
    }

    fn instantiate_binders_existentially(
        &mut self,
        interner: &I,
        arg: &Binders<Goal<I>>,
    ) -> Goal<I> {
        self.infer.instantiate_binders_existentially(interner, arg)
    }

    fn debug_ex_clause<'v>(&mut self, interner: &I, value: &'v ExClause<I>) -> Box<dyn Debug + 'v> {
        Box::new(DeepNormalizer::normalize_deep(
            &mut self.infer,
            interner,
            value,
        ))
    }

    fn fully_canonicalize_goal(
        &mut self,
        interner: &I,
        value: &InEnvironment<Goal<I>>,
    ) -> (UCanonical<InEnvironment<Goal<I>>>, UniverseMap) {
        let canonicalized_goal = self.infer.canonicalize(interner, value).quantified;
        let UCanonicalized {
            quantified,
            universes,
        } = self.infer.u_canonicalize(interner, &canonicalized_goal);
        (quantified, universes)
    }

    fn canonicalize_ex_clause(
        &mut self,
        interner: &I,
        value: &ExClause<I>,
    ) -> Canonical<ExClause<I>> {
        self.infer.canonicalize(interner, value).quantified
    }

    fn canonicalize_constrained_subst(
        &mut self,
        interner: &I,
        subst: Substitution<I>,
        constraints: Vec<InEnvironment<Constraint<I>>>,
    ) -> Canonical<ConstrainedSubst<I>> {
        self.infer
            .canonicalize(
                interner,
                &ConstrainedSubst {
                    subst,
                    constraints: Constraints::from_iter(interner, constraints),
                },
            )
            .quantified
    }

    fn canonicalize_answer_subst(
        &mut self,
        interner: &I,
        subst: Substitution<I>,
        constraints: Vec<InEnvironment<Constraint<I>>>,
        delayed_subgoals: Vec<InEnvironment<Goal<I>>>,
    ) -> Canonical<AnswerSubst<I>> {
        self.infer
            .canonicalize(
                interner,
                &AnswerSubst {
                    subst,
                    constraints: Constraints::from_iter(interner, constraints),
                    delayed_subgoals,
                },
            )
            .quantified
    }

    fn invert_goal(
        &mut self,
        interner: &I,
        value: &InEnvironment<Goal<I>>,
    ) -> Option<InEnvironment<Goal<I>>> {
        self.infer.invert(interner, value)
    }

    fn unify_generic_args_into_ex_clause(
        &mut self,
        interner: &I,
        environment: &Environment<I>,
        a: &GenericArg<I>,
        b: &GenericArg<I>,
        ex_clause: &mut ExClause<I>,
    ) -> Fallible<()> {
        let result = self.infer.unify(interner, environment, a, b)?;
        Ok(into_ex_clause(interner, result, ex_clause))
    }
}

/// Helper function
fn into_ex_clause<I: Interner>(
    interner: &I,
    result: UnificationResult<I>,
    ex_clause: &mut ExClause<I>,
) {
    ex_clause.subgoals.extend(
        result
            .goals
            .into_iter()
            .casted(interner)
            .map(Literal::Positive),
    );
}

trait SubstitutionExt<I: Interner> {
    fn may_invalidate(&self, interner: &I, subst: &Canonical<Substitution<I>>) -> bool;
}

impl<I: Interner> SubstitutionExt<I> for Substitution<I> {
    fn may_invalidate(&self, interner: &I, subst: &Canonical<Substitution<I>>) -> bool {
        self.iter(interner)
            .zip(subst.value.iter(interner))
            .any(|(new, current)| MayInvalidate { interner }.aggregate_generic_args(new, current))
    }
}

// This is a struct in case we need to add state at any point like in AntiUnifier
struct MayInvalidate<'i, I> {
    interner: &'i I,
}

impl<I: Interner> MayInvalidate<'_, I> {
    fn aggregate_generic_args(&mut self, new: &GenericArg<I>, current: &GenericArg<I>) -> bool {
        let interner = self.interner;
        match (new.data(interner), current.data(interner)) {
            (GenericArgData::Ty(ty1), GenericArgData::Ty(ty2)) => self.aggregate_tys(ty1, ty2),
            (GenericArgData::Lifetime(l1), GenericArgData::Lifetime(l2)) => {
                self.aggregate_lifetimes(l1, l2)
            }
            (GenericArgData::Const(c1), GenericArgData::Const(c2)) => self.aggregate_consts(c1, c2),
            (GenericArgData::Ty(_), _)
            | (GenericArgData::Lifetime(_), _)
            | (GenericArgData::Const(_), _) => panic!(
                "mismatched parameter kinds: new={:?} current={:?}",
                new, current
            ),
        }
    }

    /// Returns true if the two types could be unequal.
    fn aggregate_tys(&mut self, new: &Ty<I>, current: &Ty<I>) -> bool {
        let interner = self.interner;
        match (new.kind(interner), current.kind(interner)) {
            (_, TyKind::BoundVar(_)) => {
                // If the aggregate solution already has an inference
                // variable here, then no matter what type we produce,
                // the aggregate cannot get 'more generalized' than it
                // already is. So return false, we cannot invalidate.
                //
                // (Note that "inference variables" show up as *bound
                // variables* here, because we are looking at the
                // canonical form.)
                false
            }

            (TyKind::BoundVar(_), _) => {
                // If we see a type variable in the potential future
                // solution, we have to be conservative. We don't know
                // what type variable will wind up being! Remember
                // that the future solution could be any instantiation
                // of `ty0` -- or it could leave this variable
                // unbound, if the result is true for all types.
                //
                // (Note that "inference variables" show up as *bound
                // variables* here, because we are looking at the
                // canonical form.)
                true
            }

            (TyKind::InferenceVar(_, _), _) | (_, TyKind::InferenceVar(_, _)) => {
                panic!(
                    "unexpected free inference variable in may-invalidate: {:?} vs {:?}",
                    new, current,
                );
            }

            (TyKind::Placeholder(p1), TyKind::Placeholder(p2)) => {
                self.aggregate_placeholders(p1, p2)
            }

            (
                TyKind::Alias(AliasTy::Projection(proj1)),
                TyKind::Alias(AliasTy::Projection(proj2)),
            ) => self.aggregate_projection_tys(proj1, proj2),

            (
                TyKind::Alias(AliasTy::Opaque(opaque_ty1)),
                TyKind::Alias(AliasTy::Opaque(opaque_ty2)),
            ) => self.aggregate_opaque_ty_tys(opaque_ty1, opaque_ty2),

            (TyKind::Adt(id_a, substitution_a), TyKind::Adt(id_b, substitution_b)) => {
                self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b)
            }
            (
                TyKind::AssociatedType(id_a, substitution_a),
                TyKind::AssociatedType(id_b, substitution_b),
            ) => self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b),
            (TyKind::Scalar(scalar_a), TyKind::Scalar(scalar_b)) => scalar_a != scalar_b,
            (TyKind::Str, TyKind::Str) => false,
            (TyKind::Tuple(arity_a, substitution_a), TyKind::Tuple(arity_b, substitution_b)) => {
                self.aggregate_name_and_substs(arity_a, substitution_a, arity_b, substitution_b)
            }
            (
                TyKind::OpaqueType(id_a, substitution_a),
                TyKind::OpaqueType(id_b, substitution_b),
            ) => self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b),
            (TyKind::Slice(ty_a), TyKind::Slice(ty_b)) => self.aggregate_tys(ty_a, ty_b),
            (TyKind::FnDef(id_a, substitution_a), TyKind::FnDef(id_b, substitution_b)) => {
                self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b)
            }
            (TyKind::Ref(id_a, lifetime_a, ty_a), TyKind::Ref(id_b, lifetime_b, ty_b)) => {
                id_a != id_b
                    || self.aggregate_lifetimes(lifetime_a, lifetime_b)
                    || self.aggregate_tys(ty_a, ty_b)
            }
            (TyKind::Raw(id_a, ty_a), TyKind::Raw(id_b, ty_b)) => {
                id_a != id_b || self.aggregate_tys(ty_a, ty_b)
            }
            (TyKind::Never, TyKind::Never) => false,
            (TyKind::Array(ty_a, const_a), TyKind::Array(ty_b, const_b)) => {
                self.aggregate_tys(ty_a, ty_b) || self.aggregate_consts(const_a, const_b)
            }
            (TyKind::Closure(id_a, substitution_a), TyKind::Closure(id_b, substitution_b)) => {
                self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b)
            }
            (TyKind::Generator(id_a, substitution_a), TyKind::Generator(id_b, substitution_b)) => {
                self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b)
            }
            (
                TyKind::GeneratorWitness(id_a, substitution_a),
                TyKind::GeneratorWitness(id_b, substitution_b),
            ) => self.aggregate_name_and_substs(id_a, substitution_a, id_b, substitution_b),
            (TyKind::Foreign(id_a), TyKind::Foreign(id_b)) => id_a != id_b,
            (TyKind::Error, TyKind::Error) => false,

            (_, _) => true,
        }
    }

    /// Returns true if the two consts could be unequal.    
    fn aggregate_lifetimes(&mut self, _: &Lifetime<I>, _: &Lifetime<I>) -> bool {
        true
    }

    /// Returns true if the two consts could be unequal.
    fn aggregate_consts(&mut self, new: &Const<I>, current: &Const<I>) -> bool {
        let interner = self.interner;
        let ConstData {
            ty: new_ty,
            value: new_value,
        } = new.data(interner);
        let ConstData {
            ty: current_ty,
            value: current_value,
        } = current.data(interner);

        if self.aggregate_tys(new_ty, current_ty) {
            return true;
        }

        match (new_value, current_value) {
            (_, ConstValue::BoundVar(_)) => {
                // see comment in aggregate_tys
                false
            }

            (ConstValue::BoundVar(_), _) => {
                // see comment in aggregate_tys
                true
            }

            (ConstValue::InferenceVar(_), _) | (_, ConstValue::InferenceVar(_)) => {
                panic!(
                    "unexpected free inference variable in may-invalidate: {:?} vs {:?}",
                    new, current,
                );
            }

            (ConstValue::Placeholder(p1), ConstValue::Placeholder(p2)) => {
                self.aggregate_placeholders(p1, p2)
            }

            (ConstValue::Concrete(c1), ConstValue::Concrete(c2)) => {
                !c1.const_eq(new_ty, c2, interner)
            }

            // Only variants left are placeholder = concrete, which always fails
            (ConstValue::Placeholder(_), _) | (ConstValue::Concrete(_), _) => true,
        }
    }

    fn aggregate_placeholders(
        &mut self,
        new: &PlaceholderIndex,
        current: &PlaceholderIndex,
    ) -> bool {
        new != current
    }

    fn aggregate_projection_tys(
        &mut self,
        new: &ProjectionTy<I>,
        current: &ProjectionTy<I>,
    ) -> bool {
        let ProjectionTy {
            associated_ty_id: new_name,
            substitution: new_substitution,
        } = new;
        let ProjectionTy {
            associated_ty_id: current_name,
            substitution: current_substitution,
        } = current;

        self.aggregate_name_and_substs(
            new_name,
            new_substitution,
            current_name,
            current_substitution,
        )
    }

    fn aggregate_opaque_ty_tys(&mut self, new: &OpaqueTy<I>, current: &OpaqueTy<I>) -> bool {
        let OpaqueTy {
            opaque_ty_id: new_name,
            substitution: new_substitution,
        } = new;
        let OpaqueTy {
            opaque_ty_id: current_name,
            substitution: current_substitution,
        } = current;

        self.aggregate_name_and_substs(
            new_name,
            new_substitution,
            current_name,
            current_substitution,
        )
    }

    fn aggregate_name_and_substs<N>(
        &mut self,
        new_name: N,
        new_substitution: &Substitution<I>,
        current_name: N,
        current_substitution: &Substitution<I>,
    ) -> bool
    where
        N: Copy + Eq + Debug,
    {
        let interner = self.interner;
        if new_name != current_name {
            return true;
        }

        let name = new_name;

        assert_eq!(
            new_substitution.len(interner),
            current_substitution.len(interner),
            "does {:?} take {} substitution or {}? can't both be right",
            name,
            new_substitution.len(interner),
            current_substitution.len(interner)
        );

        new_substitution
            .iter(interner)
            .zip(current_substitution.iter(interner))
            .any(|(new, current)| self.aggregate_generic_args(new, current))
    }
}
