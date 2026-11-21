import importlib.util
import pathlib
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('catalog', pathlib.Path(__file__).with_name('i18n-catalog.py'))
catalog = importlib.util.module_from_spec(spec)
spec.loader.exec_module(catalog)


class CatalogTests(unittest.TestCase):
    def test_rust_unicode_and_format_fields(self):
        self.assertEqual(catalog.decode(r'"Hello \u{00B7} world"'), 'Hello · world')
        self.assertEqual(catalog.fields('Value: {n}, {} {{ok}}'), ['{n}', '{}'])

    def test_collects_translations_conflicts_and_missing(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = pathlib.Path(tmp)
            (root / 'test.rs').write_text('pt!("r", "Open", "Abrir"); pt!("r", "Open", "Aberto"); t!("r", "Close");', encoding='utf-8')
            values, required, invalid = catalog.extract(root)
            self.assertEqual(values['Open'], {'Abrir', 'Aberto'})
            self.assertEqual(required, {'Open', 'Close'})
            self.assertEqual(invalid, [])


if __name__ == '__main__':
    unittest.main()
