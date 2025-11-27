import sys
import os
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))
from core.database import DatabaseManager
import logging

logging.basicConfig(level=logging.INFO)

DB_PATH = os.path.abspath(os.path.join(os.path.dirname(__file__), '../../vfx_launcher.db'))

def main():
    db = DatabaseManager(DB_PATH)
    name = 'Test Project'
    path = 'Z:/test_project'
    print(f"Adding project: name={name}, path={path}")
    project_id = db.create_project(name, path)
    print(f"Created project with id: {project_id}")
    projects = db.get_all_projects()
    print("All projects:")
    for proj in projects:
        print(proj)

if __name__ == '__main__':
    main()
