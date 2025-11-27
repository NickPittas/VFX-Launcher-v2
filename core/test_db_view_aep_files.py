"""
Test script: View AEP files for a project directly from the database.
Run this to check if AEP files are being stored and can be loaded from the DB.
"""
import sys
import os
import sys
import os
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))
from core.database import DatabaseManager

def print_aep_files_for_project(db_path, project_id):
    db = DatabaseManager(db_path)
    files = db.get_project_files(project_id)
    aep_files = [f for f in files if f.get('filetype') == 'aep']
    print(f"AEP files for project {project_id}:")
    for f in aep_files:
        print(f"- {f.get('filename')} | version: {f.get('version')} | path: {f.get('filepath')}")
    if not aep_files:
        print("No AEP files found.")

if __name__ == "__main__":
    # Example usage: python test_db_view_aep_files.py <project_id>
    db_path = os.path.abspath(os.path.join(os.path.dirname(__file__), '../vfx_launcher.db'))
    if len(sys.argv) < 2:
        print("Usage: python test_db_view_aep_files.py <project_id>")
        sys.exit(1)
    project_id = sys.argv[1]
    print_aep_files_for_project(db_path, project_id)
