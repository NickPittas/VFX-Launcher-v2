import os
import json

class FavoritesManager:
    """
    Manages per-user favorites for projects using a JSON file in the user's config directory.
    """
    def __init__(self, username, config_dir=None):
        self.username = username
        self.config_dir = config_dir or os.path.expanduser(os.path.join('~', '.vfx_launcher'))
        os.makedirs(self.config_dir, exist_ok=True)
        self.fav_path = os.path.join(self.config_dir, f"favorites_{self.username}.json")
        self.favorites = set()
        self._load()

    def _load(self):
        if os.path.exists(self.fav_path):
            try:
                with open(self.fav_path, 'r', encoding='utf-8') as f:
                    self.favorites = set(json.load(f))
            except Exception:
                self.favorites = set()
        else:
            self.favorites = set()

    def save(self):
        try:
            with open(self.fav_path, 'w', encoding='utf-8') as f:
                json.dump(list(self.favorites), f, indent=2)
        except Exception:
            pass

    def is_favorite(self, project_id_or_path):
        return project_id_or_path in self.favorites

    def add_favorite(self, project_id_or_path):
        self.favorites.add(project_id_or_path)
        self.save()

    def remove_favorite(self, project_id_or_path):
        self.favorites.discard(project_id_or_path)
        self.save()

    def get_favorites(self):
        return list(self.favorites)
