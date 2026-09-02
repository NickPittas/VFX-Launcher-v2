import sqlite3
import os

try:
    from core.config import ConfigManager
    DB_PATH = ConfigManager().get_database_path()
except Exception:
    # Fallback: database in the repo root next to this script
    DB_PATH = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'vfx_launcher.db')

def main():
    db_path = DB_PATH
    if not os.path.exists(db_path):
        print(f"Database file not found: {db_path}")
        return
    conn = sqlite3.connect(db_path)
    cur = conn.cursor()
    print(f"Database: {db_path}")
    print("project_id | filename | filepath | version | filetype")
    print("-" * 80)
    try:
        cur.execute("SELECT project_id, filename, filepath, version, filetype FROM project_files ORDER BY project_id, filename")
        rows = cur.fetchall()
        if not rows:
            print("No files found in project_files table.")
        for row in rows:
            print(" | ".join(str(x) if x is not None else '' for x in row))
    except Exception as e:
        print(f"Error querying database: {e}")
    finally:
        conn.close()

if __name__ == "__main__":
    main()
