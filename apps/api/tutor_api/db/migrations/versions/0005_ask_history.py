"""The conversation a question followed (LLD §9, §11; as built: see db/tables.py): the ids of the
earlier answers sent with it (`AskRequest.history`), oldest first, so a reviewer can read a follow-up
in its thread.

Revision ID: 0005
Create Date: 2026-05-25
"""

from alembic import op

revision = "0005"
down_revision = "0004"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.execute("ALTER TABLE asks ADD COLUMN history uuid[]")


def downgrade() -> None:
    op.execute("ALTER TABLE asks DROP COLUMN history")
