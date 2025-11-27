import sqlite3
import os

def main():
    db_path = 'vfx_launcher.db'
    if not os.path.exists(db_path):
        print(f"Database file not found: {db_path}")
        return
    conn = sqlite3.connect(db_path)
    cur = conn.cursor()
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
