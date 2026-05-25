"""The grounded tutor (LLD §9): the context slice and answer reading live in circuit-core; this
package asks the model and appends what the server owns (the safety note)."""

from .service import MAX_HISTORY, Asked, Turn, Tutor

__all__ = ["MAX_HISTORY", "Asked", "Turn", "Tutor"]
