import os

def test_open_configurator_cli_exists():
    path = os.path.join(os.path.dirname(__file__), '..', 'tools', 'open_configurator_cli.py')
    assert os.path.exists(path), 'open_configurator_cli.py must exist'

def test_rollback_helper_exists():
    path = os.path.join(os.path.dirname(__file__), '..', 'tools', 'rollback_continue_config.py')
    assert os.path.exists(path), 'rollback_continue_config.py must exist'

def test_migration_script_exists():
    path = os.path.join(os.path.dirname(__file__), '..', 'tools', 'migrate_configurator.py')
    assert os.path.exists(path), 'migrate_configurator.py must exist'
