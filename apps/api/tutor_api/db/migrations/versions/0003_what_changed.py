""""What changed?" answers in `asks` (LLD §9, §11; as built: see db/tables.py): each row's `kind`,
and for an explained edit the rev it started from.

Revision ID: 0003
Create Date: 2026-02-15
"""

from alembic import op

revision = "0003"
down_revision = "0002"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.execute("ALTER TABLE asks ADD COLUMN kind text NOT NULL DEFAULT 'ask' CHECK (kind IN ('ask','what_changed'))")
    op.execute("ALTER TABLE asks ADD COLUMN from_rev bigint")


def downgrade() -> None:
    op.execute("ALTER TABLE asks DROP COLUMN from_rev")
    op.execute("ALTER TABLE asks DROP COLUMN kind")
