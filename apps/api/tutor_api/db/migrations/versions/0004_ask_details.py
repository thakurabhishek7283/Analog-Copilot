"""What the student test sessions and later audits need from each answer in `asks` (LLD §9, §11,
§16; as built: see db/tables.py): the level and effort the learner chose, the context the model was
given (so an answer's numbers can be checked against it later), and how long it took.

Revision ID: 0004
Create Date: 2026-03-08
"""

from alembic import op

revision = "0004"
down_revision = "0003"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.execute("ALTER TABLE asks ADD COLUMN level text")
    op.execute("ALTER TABLE asks ADD COLUMN effort text CHECK (effort IN ('low','high'))")
    op.execute("ALTER TABLE asks ADD COLUMN context text")
    op.execute("ALTER TABLE asks ADD COLUMN first_token_ms integer")
    op.execute("ALTER TABLE asks ADD COLUMN ms integer")


def downgrade() -> None:
    for column in ("ms", "first_token_ms", "context", "effort", "level"):
        op.execute(f"ALTER TABLE asks DROP COLUMN {column}")
