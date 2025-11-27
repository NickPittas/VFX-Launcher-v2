"""
Database Cleaning Script: Remove Orphan/Misplaced Projects

This script removes entries from the 'projects' table that are not true configured project roots.
It also removes any files associated with those orphan projects.

Usage:
    python clean_orphan_projects.py

Make sure to back up your database before running this script!
"""

import sqlite3
import os
import logging

# --- CONFIGURE THESE ---
DB_PATH = os.path.abspath(os.path.join(os.path.dirname(__file__), 'vfx_launcher.db'))
# List of valid project root paths (should match your config)
VALID_PROJECT_ROOTS = [
    # Example: r"H:/20089_JEFFMILLS_2021-12-16",
    # Add all valid project root absolute paths here
]

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("db_cleaner")

def get_all_projects(conn):
    cur = conn.execute("SELECT id, name, path FROM projects")
    return cur.fetchall()

def delete_project(conn, project_id):
    conn.execute("DELETE FROM project_files WHERE project_id=?", (project_id,))
    conn.execute("DELETE FROM projects WHERE id=?", (project_id,))
    logger.info(f"Deleted orphan project id={project_id}")

def main():
    if not VALID_PROJECT_ROOTS:
        logger.error("VALID_PROJECT_ROOTS is empty. Please populate it with your project roots.")
        return
    conn = sqlite3.connect(DB_PATH)
    try:
        projects = get_all_projects(conn)
        orphans = [p for p in projects if p[2] not in VALID_PROJECT_ROOTS]
        logger.info(f"Found {len(orphans)} orphan/misplaced projects.")
        for proj in orphans:
            delete_project(conn, proj[0])
        conn.commit()
        logger.info("Database cleanup complete.")
    finally:
        conn.close()

if __name__ == "__main__":
    main()
