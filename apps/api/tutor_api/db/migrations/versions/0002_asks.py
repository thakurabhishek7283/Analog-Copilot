"""The tutor's answers (LLD §9, §11; as built: see db/tables.py).

Revision ID: 0002
Create Date: 2026-02-09
"""

from alembic import op

revision = "0002"
down_revision = "0001"
branch_labels = None
depends_on = None

UP = """
CREATE TABLE asks (
  id uuid PRIMARY KEY, project_id uuid NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  user_id uuid NOT NULL REFERENCES users(id), rev bigint NOT NULL,
  selection jsonb, question text NOT NULL, answer text NOT NULL,
  mode text NOT NULL CHECK (mode IN ('explain','socratic')),
  refs_valid int NOT NULL DEFAULT 0, refs_invalid int NOT NULL DEFAULT 0,
  model text, in_tokens int NOT NULL DEFAULT 0, out_tokens int NOT NULL DEFAULT 0,
  feedback smallint CHECK (feedback IN (-1, 1)), created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX asks_project ON asks (project_id, created_at);
CREATE INDEX asks_user_recent ON asks (user_id, created_at DESC);
"""


def upgrade() -> None:
    for statement in UP.split(";\n"):
        if statement.strip():
            op.execute(statement)


def downgrade() -> None:
    op.execute("DROP TABLE asks")
