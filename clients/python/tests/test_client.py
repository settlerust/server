import unittest
from settlers_client import Client
class ClientTest(unittest.TestCase):
    def test_prepared_command_is_stable_for_retries(self):
        request = Client.prepare(8, {'type': 'act', 'action': {'type': 'roll'}})
        self.assertEqual(request['expected_version'], 8)
        self.assertEqual(len(request['request_id']), 36)
    def test_generated_types_import(self):
        from settlers_client.types import ServerFrame, Resources
        self.assertIsNotNone(ServerFrame)
        self.assertIsNotNone(Resources)
if __name__ == '__main__': unittest.main()
