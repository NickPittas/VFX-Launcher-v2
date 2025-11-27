from PySide6.QtWidgets import QStyledItemDelegate, QComboBox
from PySide6.QtCore import Qt

class VersionDelegate(QStyledItemDelegate):
    """
    Custom delegate to provide a dropdown for version selection in the Version column.
    """
    def __init__(self, parent=None, version_options=None):
        super().__init__(parent)
        self.version_options = version_options or []

    def createEditor(self, parent, option, index):
        combo = QComboBox(parent)
        # If the model provides version options per row, use them
        options = index.data(Qt.UserRole + 4) or self.version_options
        combo.addItems([str(v) for v in options])
        return combo

    def setEditorData(self, editor, index):
        value = index.data(Qt.EditRole)
        i = editor.findText(str(value))
        if i >= 0:
            editor.setCurrentIndex(i)

    def setModelData(self, editor, model, index):
        model.setData(index, editor.currentText(), Qt.EditRole)

    def updateEditorGeometry(self, editor, option, index):
        editor.setGeometry(option.rect)
