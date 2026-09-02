"""
Database Management Module

Purpose:
    Handles SQLite database initialization, thread-safe connection management,
    and schema management. Provides reusable CRUD functions for all database
    operations.

Requirements:
    - Automatically create the database and tables if not present
    - Robust error handling and logging
    - Efficient connection management
"""

import os
import sqlite3
import logging
import threading
import weakref
from contextlib import contextmanager
import time

# Configure logger
logger = logging.getLogger(__name__)


def _close_connections(thread_connections):
    """
    Close every per-thread connection tracked in thread_connections.

    Registered as a weakref finalizer so connections are closed when the
    owning DatabaseManager is garbage-collected. Connections are created
    with check_same_thread=False, so this may run on any thread. Individual
    failures are logged and swallowed so one bad connection does not
    prevent the rest from being closed.
    """
    for thread_id, connection in list(thread_connections.items()):
        try:
            connection.close()
        except Exception as e:
            logger.warning(f"Failed to close connection for thread {thread_id}: {e}")

class DatabaseManager:
    """
    Handles all database operations including initialization, thread-safe
    connection management, and provides methods for CRUD operations.
    """
    
    def __init__(self, db_path):
        """
        Initialize the database manager with the specified database path.
        
        Args:
            db_path (str): Path to the SQLite database file
        """
        self.db_path = db_path

        # Track which thread owns each connection (per-thread connection cache)
        self._thread_connections = {}

        # Close all per-thread connections when this manager is collected.
        # Connections use check_same_thread=False so any thread may close them.
        self._finalizer = weakref.finalize(
            self, _close_connections, self._thread_connections
        )
        
        # Ensure database directory exists
        os.makedirs(os.path.dirname(os.path.abspath(db_path)), exist_ok=True)
        
        # Initialize database if it doesn't exist
        self._initialize_database()
    
    def _initialize_database(self):
        """
        Create database tables if they don't exist.
        Uses schema defined in SQLite_schema.md
        """
        try:
            # Create users table
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    username TEXT UNIQUE NOT NULL,
                    is_admin INTEGER DEFAULT 0,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )
            """)
            
            # Create projects table
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS projects (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL,
                    path TEXT NOT NULL UNIQUE,
                    added_by TEXT,
                    added_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )
            """)
            
            # Create project_files table
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS project_files (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    project_id INTEGER,
                    filename TEXT,
                    filepath TEXT,
                    version TEXT,
                    filetype TEXT,
                    last_modified TIMESTAMP,
                    FOREIGN KEY(project_id) REFERENCES projects(id)
                )
            """)
            
            # Create recent_projects table
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS recent_projects (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER,
                    project_id INTEGER,
                    accessed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY(user_id) REFERENCES users(id),
                    FOREIGN KEY(project_id) REFERENCES projects(id)
                )
            """)
            
            # Create favorite_projects table
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS favorite_projects (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER,
                    project_id INTEGER,
                    FOREIGN KEY(user_id) REFERENCES users(id),
                    FOREIGN KEY(project_id) REFERENCES projects(id)
                )
            """)

            # Create file_access_log table to track when users open files
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS file_access_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    file_id INTEGER,
                    user_id INTEGER,
                    accessed_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY(file_id) REFERENCES project_files(id),
                    FOREIGN KEY(user_id) REFERENCES users(id)
                )
            """)

            # Create user_activity_log table to record user actions
            self._execute_query("""
                CREATE TABLE IF NOT EXISTS user_activity_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    username TEXT,
                    action_type TEXT,
                    description TEXT,
                    timestamp TEXT DEFAULT (datetime('now','localtime'))
                )
            """)

            # Check if default admin user exists, create if not
            self._execute_query("""
                INSERT OR IGNORE INTO users (username, is_admin)
                VALUES ('admin', 1)
            """)
            
            logger.info("Database initialized successfully")
        except Exception as e:
            logger.error(f"Error initializing database: {str(e)}")
            raise
    
    @classmethod
    def create_thread_safe_manager(cls, db_path):
        """
        Create a new database manager instance for use in a separate thread.
        
        This method should be called by worker threads that need to access the database.
        Each thread should have its own database manager instance to avoid SQLite thread safety issues.
        
        Args:
            db_path (str): Path to the SQLite database file
        
        Returns:
            DatabaseManager: A new database manager instance safe for use in the current thread
        """
        logger.info(f"Creating thread-safe database manager for thread ID: {os.getpid()}")
        return cls(db_path)
        
    @contextmanager
    def get_connection(self):
        """
        Get the database connection for the current thread.

        Connections are cached per thread and reused for the lifetime of
        this manager. Each connection is created with check_same_thread=False
        so the weakref finalizer may close it from any thread.

        Yields:
            sqlite3.Connection: SQLite connection object
        """
        thread_id = threading.get_ident()

        if thread_id in self._thread_connections:
            connection = self._thread_connections[thread_id]
        else:
            # Create a new connection for this thread
            connection = sqlite3.connect(self.db_path, check_same_thread=False)
            # Enable foreign key support
            connection.execute("PRAGMA foreign_keys = ON")
            # Ensure rows are dict-like for all queries
            connection.row_factory = sqlite3.Row
            # Store this connection as the thread's connection
            self._thread_connections[thread_id] = connection

        try:
            yield connection
        except Exception as e:
            logger.error(f"Database connection error: {str(e)}")
            raise
    
    def _execute_query(self, query, params=None):
        """
        Execute a query with optional parameters.
        
        Args:
            query (str): SQL query to execute
            params (tuple, dict, optional): Parameters for the query
            
        Returns:
            list: Query results if applicable
        """
        with self.get_connection() as conn:
            cursor = conn.cursor()
            try:
                if params:
                    cursor.execute(query, params)
                else:
                    cursor.execute(query)
                conn.commit()
                return cursor.fetchall()
            except sqlite3.Error as e:
                conn.rollback()
                logger.error(f"Query execution error: {str(e)}")
                logger.error(f"Query: {query}")
                logger.error(f"Params: {params}")
                raise
    
    def remove_project_file(self, file_id):
        """
        Remove a project file from the database by its id.
        Args:
            file_id (int): ID of the file to remove
        """
        try:
            self._execute_query("DELETE FROM project_files WHERE id = ?", (file_id,))
            logger.info(f"Removed project file with id {file_id}")
        except Exception as e:
            logger.error(f"Error removing project file {file_id}: {str(e)}")
            raise

    def _execute_many(self, query, params_list):
        """
        Execute a query with multiple parameter sets.
        
        Args:
            query (str): SQL query to execute
            params_list (list): List of parameter tuples/dicts
            
        Returns:
            int: Number of rows affected
        """
        with self.get_connection() as conn:
            cursor = conn.cursor()
            try:
                cursor.executemany(query, params_list)
                conn.commit()
                return cursor.rowcount
            except sqlite3.Error as e:
                conn.rollback()
                logger.error(f"Query execution error: {str(e)}")
                logger.error(f"Query: {query}")
                logger.error(f"Params list length: {len(params_list)}")
                raise
    
    def _execute_transaction(self, operations):
        """
        Execute multiple statements as a single atomic transaction on one connection.

        Args:
            operations (list[tuple[str, tuple]]): (query, params) pairs executed in order

        Raises:
            sqlite3.Error: On failure, after rolling back all changes
        """
        with self.get_connection() as conn:
            cursor = conn.cursor()
            try:
                cursor.execute("BEGIN IMMEDIATE")
                for query, params in operations:
                    cursor.execute(query, params)
                conn.commit()
            except sqlite3.Error as e:
                conn.rollback()
                logger.error(f"Transaction execution error: {str(e)}")
                for query, params in operations:
                    logger.error(f"Transaction statement: {query}")
                raise

    # User CRUD operations
    def create_user(self, username, is_admin=0):
        """
        Create a new user.
        
        Args:
            username (str): Unique username
            is_admin (int): 1 if user is admin, 0 otherwise
            
        Returns:
            int: ID of the created user
        """
        try:
            result = self._execute_query(
                "INSERT INTO users (username, is_admin) VALUES (?, ?)",
                (username, is_admin)
            )
            return self.get_user_by_username(username)['id']
        except sqlite3.IntegrityError:
            logger.warning(f"User '{username}' already exists")
            return self.get_user_by_username(username)['id']
    
    def get_user_by_id(self, user_id):
        """
        Get user by ID.
        
        Args:
            user_id (int): User ID
            
        Returns:
            dict: User data or None if not found
        """
        result = self._execute_query(
            "SELECT * FROM users WHERE id = ?",
            (user_id,)
        )
        return dict(result[0]) if result else None
    
    def get_user_by_username(self, username):
        """
        Get user by username.
        
        Args:
            username (str): Username
            
        Returns:
            dict: User data or None if not found
        """
        result = self._execute_query(
            "SELECT * FROM users WHERE username = ?",
            (username,)
        )
        return dict(result[0]) if result else None
    
    def get_all_users(self):
        """
        Get all users.
        
        Returns:
            list: List of user dictionaries
        """
        result = self._execute_query("SELECT * FROM users ORDER BY username")
        return [dict(row) for row in result]
    
    def update_user(self, user_id, username=None, is_admin=None):
        """
        Update user information.
        
        Args:
            user_id (int): User ID
            username (str, optional): New username
            is_admin (int, optional): New admin status
            
        Returns:
            bool: True if update successful
        """
        current_user = self.get_user_by_id(user_id)
        if not current_user:
            logger.warning(f"User with ID {user_id} not found")
            return False
        
        if username is None:
            username = current_user['username']
        if is_admin is None:
            is_admin = current_user['is_admin']
        
        self._execute_query(
            "UPDATE users SET username = ?, is_admin = ? WHERE id = ?",
            (username, is_admin, user_id)
        )
        return True
    
    def delete_user(self, user_id):
        """
        Delete a user.
        
        Args:
            user_id (int): User ID
            
        Returns:
            bool: True if deletion successful
        """
        self._execute_query("DELETE FROM users WHERE id = ?", (user_id,))
        return True
    
    # Project CRUD operations
    def create_project(self, name, path, added_by=None):
        """
        Create a new project.
        
        Args:
            name (str): Project name
            path (str): Project path
            added_by (str, optional): Username who added the project
            
        Returns:
            int: ID of the created project
        """
        try:
            self._execute_query(
                "INSERT INTO projects (name, path, added_by) VALUES (?, ?, ?)",
                (name, path, added_by)
            )
            result = self._execute_query(
                "SELECT id FROM projects WHERE path = ?",
                (path,)
            )
            return result[0]['id'] if result else None
        except sqlite3.IntegrityError:
            logger.warning(f"Project with path '{path}' already exists")
            result = self._execute_query(
                "SELECT id FROM projects WHERE path = ?",
                (path,)
            )
            return result[0]['id'] if result else None
    
    def get_project_by_id(self, project_id):
        """
        Get project by ID.
        
        Args:
            project_id (int): Project ID
            
        Returns:
            dict: Project data or None if not found
        """
        result = self._execute_query(
            "SELECT * FROM projects WHERE id = ?",
            (project_id,)
        )
        return dict(result[0]) if result else None
    
    def get_project_by_path(self, path):
        """
        Get project by path.
        
        Args:
            path (str): Project path
            
        Returns:
            dict: Project data or None if not found
        """
        result = self._execute_query(
            "SELECT * FROM projects WHERE path = ?",
            (path,)
        )
        return dict(result[0]) if result else None
    
    def get_all_projects(self):
        """
        Get all projects.
        
        Returns:
            list: List of project dictionaries
        """
        logger.info(f"[DIAG] get_all_projects: db_path={self.db_path}")
        result = self._execute_query("SELECT id, name, path, added_by, added_at FROM projects ORDER BY name")
        logger.info(f"[DIAG] get_all_projects: raw result={result}")
        dicts = [dict(row) for row in result]
        logger.info(f"[DIAG] get_all_projects: dicts={dicts}")
        return dicts
    
    def update_project(self, project_id, name=None, path=None):
        """
        Update project information.
        
        Args:
            project_id (int): Project ID
            name (str, optional): New project name
            path (str, optional): New project path
            
        Returns:
            bool: True if update successful
        """
        current_project = self.get_project_by_id(project_id)
        if not current_project:
            logger.warning(f"Project with ID {project_id} not found")
            return False
        
        if name is None:
            name = current_project['name']
        if path is None:
            path = current_project['path']
        
        try:
            self._execute_query(
                "UPDATE projects SET name = ?, path = ? WHERE id = ?",
                (name, path, project_id)
            )
            return True
        except sqlite3.IntegrityError:
            logger.warning(f"Cannot update project - path '{path}' already exists")
            return False
    
    def update_project_folders(self, project_id, folders):
        """
        Store the list of folders scanned for a project.

        Adds the folders column to the projects table if it does not exist
        yet, then stores the folder paths newline-separated.

        Args:
            project_id (int): Project ID
            folders (list[str]): Folder paths associated with the project

        Returns:
            bool: True if successful, False otherwise
        """
        try:
            with self.get_connection() as conn:
                try:
                    conn.execute("ALTER TABLE projects ADD COLUMN folders TEXT")
                    conn.commit()
                except sqlite3.OperationalError as e:
                    # Column was already added on a previous call
                    if "duplicate column" not in str(e).lower():
                        raise
            self._execute_query(
                "UPDATE projects SET folders = ? WHERE id = ?",
                ("\n".join(folders), project_id)
            )
            return True
        except Exception as e:
            logger.error(f"Failed to update folders for project_id={project_id}: {e}")
            return False

    def delete_project(self, project_id):
        """
        Delete a project and all associated records in a single transaction.

        file_access_log references project_files without an ON DELETE
        cascade, so access-log rows must be removed before project_files.

        Args:
            project_id (int): Project ID

        Returns:
            bool: True if deletion successful
        """
        try:
            logger.info(f"[DB] Deleting project_id={project_id}")
            self._execute_transaction([
                (
                    "DELETE FROM file_access_log WHERE file_id IN "
                    "(SELECT id FROM project_files WHERE project_id = ?)",
                    (project_id,)
                ),
                ("DELETE FROM project_files WHERE project_id = ?", (project_id,)),
                ("DELETE FROM recent_projects WHERE project_id = ?", (project_id,)),
                ("DELETE FROM favorite_projects WHERE project_id = ?", (project_id,)),
                ("DELETE FROM projects WHERE id = ?", (project_id,)),
            ])
            logger.info(f"[DB] Successfully deleted project_id={project_id}")
            return True
        except Exception as e:
            logger.error(f"[DB] Failed to delete project_id={project_id}: {e}")
            return False
    
    # Project Files operations
    def add_project_file(self, project_id, filename, filepath, version, filetype, last_modified):
        """
        Add a file to a project.
        
        Args:
            project_id (int): Project ID
            filename (str): File name
            filepath (str): Full file path
            version (str): File version
            filetype (str): File type (nk or aep)
            last_modified (float): Last modified timestamp
            
        Returns:
            int: ID of the created file record
        """
        # Check if file already exists by filepath
        existing = self._execute_query(
            "SELECT id FROM project_files WHERE filepath = ?",
            (filepath,)
        )
        
        if existing:
            # Update existing record, including project_id and filetype (fix for multi-project overlap)
            self._execute_query(
                """
                UPDATE project_files 
                SET project_id = ?, filename = ?, version = ?, filetype = ?, last_modified = ?
                WHERE filepath = ?
                """,
                (project_id, filename, version, filetype, last_modified, filepath)
            )
            logger.info(f"[DB] Updated project_file: project_id={project_id}, filename={filename}, filepath={filepath}, version={version}, filetype={filetype}, last_modified={last_modified}")
            return existing[0]['id']
        else:
            # Create new record
            self._execute_query(
                """
                INSERT INTO project_files 
                (project_id, filename, filepath, version, filetype, last_modified)
                VALUES (?, ?, ?, ?, ?, ?)
                """,
                (project_id, filename, filepath, version, filetype, last_modified)
            )
            logger.info(f"[DB] Inserted project_file: project_id={project_id}, filename={filename}, filepath={filepath}, version={version}, filetype={filetype}, last_modified={last_modified}")
            result = self._execute_query(
                "SELECT id FROM project_files WHERE filepath = ?",
                (filepath,)
            )
            return result[0]['id'] if result else None
    
    def get_project_files(self, project_id):
        """
        Get all files for a project.
        
        Args:
            project_id (int): Project ID
            
        Returns:
            list: List of file dictionaries
        """
        result = self._execute_query(
            """
            SELECT * FROM project_files 
            WHERE project_id = ?
            ORDER BY filename, version DESC
            """,
            (project_id,)
        )
        return [dict(row) for row in result]
    
    def get_file_versions(self, project_id, filename_base):
        """
        Get all versions of a specific file.
        
        Args:
            project_id (int): Project ID
            filename_base (str): Base filename without version
            
        Returns:
            list: List of file dictionaries
        """
        escaped_base = (
            filename_base
            .replace("/", "//")
            .replace("%", "/%")
            .replace("_", "/_")
        )
        result = self._execute_query(
            """
            SELECT * FROM project_files
            WHERE project_id = ?
              AND (
                  filename LIKE ? ESCAPE '/'
                  OR filename = ?
                  OR filename = ?
              )
            ORDER BY version DESC
            """,
            (
                project_id,
                f"{escaped_base}/_v%",
                f"{filename_base}.nk",
                f"{filename_base}.aep",
            )
        )
        return [dict(row) for row in result]
    
    # Recent projects operations
    def add_recent_project(self, user_id, project_id):
        """
        Add or update a project in user's recent list.
        
        Args:
            user_id (int): User ID
            project_id (int): Project ID
            
        Returns:
            bool: True if successful
        """
        # Delete existing record if present (will be re-added with current timestamp)
        self._execute_query(
            "DELETE FROM recent_projects WHERE user_id = ? AND project_id = ?",
            (user_id, project_id)
        )
        
        # Add new record
        self._execute_query(
            "INSERT INTO recent_projects (user_id, project_id) VALUES (?, ?)",
            (user_id, project_id)
        )
        return True
    
    def get_recent_projects(self, user_id, limit=10):
        """
        Get recent projects for a user.
        
        Args:
            user_id (int): User ID
            limit (int): Maximum number of projects to return
            
        Returns:
            list: List of project dictionaries with access timestamp
        """
        result = self._execute_query(
            """
            SELECT p.*, r.accessed_at
            FROM recent_projects r
            JOIN projects p ON r.project_id = p.id
            WHERE r.user_id = ?
            ORDER BY r.accessed_at DESC
            LIMIT ?
            """,
            (user_id, limit)
        )
        return [dict(row) for row in result]

    # File Access Tracking operations
    def log_file_access(self, file_id, user_id):
        """
        Log when a user opens a file.

        Args:
            file_id (int): File ID from project_files table
            user_id (int): User ID

        Returns:
            bool: True if successful
        """
        try:
            self._execute_query(
                "INSERT INTO file_access_log (file_id, user_id) VALUES (?, ?)",
                (file_id, user_id)
            )
            logger.info(f"[DB] Logged file access: file_id={file_id}, user_id={user_id}")
            return True
        except Exception as e:
            logger.error(f"[DB] Failed to log file access: {e}")
            return False

    def get_file_last_access(self, file_id):
        """
        Get the last access information for a file.

        Args:
            file_id (int): File ID from project_files table

        Returns:
            dict: Dictionary with 'accessed_at' and 'username', or None if never accessed
        """
        result = self._execute_query(
            """
            SELECT fal.accessed_at, u.username
            FROM file_access_log fal
            JOIN users u ON fal.user_id = u.id
            WHERE fal.file_id = ?
            ORDER BY fal.accessed_at DESC
            LIMIT 1
            """,
            (file_id,)
        )
        return dict(result[0]) if result else None

    def get_files_with_access_info(self, project_id):
        """
        Get all files for a project with their last access information.

        Args:
            project_id (int): Project ID

        Returns:
            list: List of file dictionaries with 'last_opened' and 'opened_by' fields
        """
        result = self._execute_query(
            """
            SELECT
                pf.*,
                fal.accessed_at as last_opened,
                u.username as opened_by
            FROM project_files pf
            LEFT JOIN (
                SELECT file_id, MAX(accessed_at) as accessed_at, user_id
                FROM file_access_log
                GROUP BY file_id
            ) fal ON pf.id = fal.file_id
            LEFT JOIN users u ON fal.user_id = u.id
            WHERE pf.project_id = ?
            ORDER BY pf.filename, pf.version DESC
            """,
            (project_id,)
        )
        return [dict(row) for row in result]

    # Favorite projects operations
    def add_favorite_project(self, user_id, project_id):
        """
        Add a project to user's favorites.
        
        Args:
            user_id (int): User ID
            project_id (int): Project ID
            
        Returns:
            bool: True if successful
        """
        # Check if already a favorite
        existing = self._execute_query(
            "SELECT id FROM favorite_projects WHERE user_id = ? AND project_id = ?",
            (user_id, project_id)
        )
        
        if not existing:
            self._execute_query(
                "INSERT INTO favorite_projects (user_id, project_id) VALUES (?, ?)",
                (user_id, project_id)
            )
        return True
    
    def remove_favorite_project(self, user_id, project_id):
        """
        Remove a project from user's favorites.
        
        Args:
            user_id (int): User ID
            project_id (int): Project ID
            
        Returns:
            bool: True if successful
        """
        self._execute_query(
            "DELETE FROM favorite_projects WHERE user_id = ? AND project_id = ?",
            (user_id, project_id)
        )
        return True
    
    def get_favorite_projects(self, user_id):
        """
        Get favorite projects for a user.
        
        Args:
            user_id (int): User ID
            
        Returns:
            list: List of project dictionaries
        """
        result = self._execute_query(
            """
            SELECT p.*
            FROM favorite_projects f
            JOIN projects p ON f.project_id = p.id
            WHERE f.user_id = ?
            ORDER BY p.name
            """,
            (user_id,)
        )
        return [dict(row) for row in result]
    
    def is_project_favorite(self, user_id, project_id):
        """
        Check if a project is in user's favorites.
        
        Args:
            user_id (int): User ID
            project_id (int): Project ID
            
        Returns:
            bool: True if project is a favorite
        """
        result = self._execute_query(
            "SELECT id FROM favorite_projects WHERE user_id = ? AND project_id = ?",
            (user_id, project_id)
        )
        return bool(result)
