"""
Configuration Manager for VFX Launcher
Handles reading configuration from app_settings.ini file
"""
import os
import sys
import configparser
import logging

logger = logging.getLogger(__name__)


def resolve_app_settings_path():
    """Path to app_settings.ini. Frozen builds (PyInstaller/AppImage) get a
    writable per-user copy seeded from the bundled file; dev runs use the repo file."""
    bundled = os.path.abspath(os.path.join(os.path.dirname(__file__), '..', 'app_settings.ini'))
    if not getattr(sys, 'frozen', False):
        return bundled
    user_copy = os.path.join(os.path.expanduser('~'), '.config', 'VFX_Launcher', 'app_settings.ini')
    if not os.path.exists(user_copy):
        os.makedirs(os.path.dirname(user_copy), exist_ok=True)
        if os.path.exists(bundled):
            import shutil
            shutil.copy(bundled, user_copy)
    return user_copy

class ConfigManager:
    """Manages application configuration"""
    
    def __init__(self, config_file=None):
        """
        Initialize configuration manager

        Args:
            config_file: Path to app_settings.ini file. If None, the active
                app_settings.ini is resolved automatically (see resolve_app_settings_path).
        """
        self.config = configparser.ConfigParser()
        self.config_file = config_file or self._find_config_file()

        if self.config_file and os.path.exists(self.config_file):
            logger.info(f"Loading configuration from: {self.config_file}")
            try:
                self.config.read(self.config_file)
            except configparser.Error as e:
                logger.warning(f"Error reading config file {self.config_file}: {e}. Using defaults.")
            self._set_defaults()
        else:
            logger.warning(f"Config file not found: {self.config_file}. Using defaults.")
            self._set_defaults()
    
    def _find_config_file(self):
        """Resolve the active app_settings.ini location"""
        return resolve_app_settings_path()
    
    def _set_defaults(self):
        """Set default configuration values"""
        # Database defaults
        if not self.config.has_section('Database'):
            self.config.add_section('Database')
        
        # Default database path in user's AppData
        default_data_dir = os.path.join(
            os.environ.get('APPDATA', os.path.expanduser('~')),
            'VFX_Launcher'
        )
        
        if not self.config.has_option('Database', 'db_path'):
            self.config.set('Database', 'db_path',
                          os.path.join(default_data_dir, 'vfx_launcher.db'))
        
        # Paths defaults
        if not self.config.has_section('Paths'):
            self.config.add_section('Paths')
        
        if not self.config.has_option('Paths', 'DataDir'):
            self.config.set('Paths', 'DataDir', default_data_dir)
    
    def get_database_path(self):
        """Get the database file path"""
        db_path = self.config.get('Database', 'db_path',
                                  fallback=os.path.join(os.path.dirname(__file__), '..', 'vfx_launcher.db'))

        # Resolve relative paths against the config file's directory so the
        # database stays beside the per-user config in frozen builds
        if not os.path.isabs(db_path):
            db_path = os.path.join(os.path.dirname(os.path.abspath(self.config_file)), db_path)
        db_path = os.path.abspath(db_path)

        # Legacy frozen builds kept the database at ~/VFX_Launcher (APPDATA on
        # Windows). If the resolved path is new but the legacy database exists,
        # keep using it so upgraded users don't appear to lose their data.
        if not os.path.exists(db_path):
            legacy_db = os.path.join(
                os.environ.get('APPDATA', os.path.expanduser('~')),
                'VFX_Launcher', 'vfx_launcher.db'
            )
            if os.path.exists(legacy_db):
                logger.info(f"Using legacy database location: {legacy_db}")
                return legacy_db

        # Ensure the directory exists
        db_dir = os.path.dirname(db_path)
        if not os.path.exists(db_dir):
            logger.info(f"Creating database directory: {db_dir}")
            os.makedirs(db_dir, exist_ok=True)

        return db_path
    
    def get_data_dir(self):
        """Get the data directory path"""
        data_dir = self.config.get('Paths', 'DataDir',
                                   fallback=os.path.join(os.path.dirname(__file__), '..'))
        
        # Ensure the directory exists
        if not os.path.exists(data_dir):
            logger.info(f"Creating data directory: {data_dir}")
            os.makedirs(data_dir, exist_ok=True)
        
        return data_dir
    
    def get_config_dir(self):
        """Get the config directory path"""
        data_dir = self.get_data_dir()
        config_dir = os.path.join(data_dir, 'config')
        
        # Ensure the directory exists
        if not os.path.exists(config_dir):
            logger.info(f"Creating config directory: {config_dir}")
            os.makedirs(config_dir, exist_ok=True)
        
        return config_dir
    
    def save(self):
        """Save configuration to file"""
        if self.config_file:
            os.makedirs(os.path.dirname(os.path.abspath(self.config_file)), exist_ok=True)
            with open(self.config_file, 'w') as f:
                self.config.write(f)
            logger.info(f"Configuration saved to: {self.config_file}")

