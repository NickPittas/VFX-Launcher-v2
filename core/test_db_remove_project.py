import sys
import os
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))
from core.database import DatabaseManager
import logging

logging.basicConfig(level=logging.INFO)

DB_PATH = os.path.abspath(os.path.join(os.path.dirname(__file__), '../../vfx_launcher.db'))

def main():
    db = DatabaseManager(DB_PATH)
    projects = db.get_all_projects()
    print("All projects before deletion:")
    for proj in projects:
        print(proj)
    if not projects:
        print("No projects to delete.")
        return
    project_id = projects[-1]['id']
    print(f"Attempting to delete project with id: {project_id}")
    result = db.delete_project(project_id)
    print(f"delete_project({project_id}) result: {result}")
    projects = db.get_all_projects()
    print("All projects after deletion:")
    for proj in projects:
        print(proj)

if __name__ == '__main__':
    main()
