"""Optional developer verification; requires Python + pexpect, not needed at runtime."""
from pathlib import Path
import tempfile
import pexpect

root=Path(__file__).resolve().parents[1]
exe=root/'bin/saeugling-linux-x86_64'
password='Disposable smoke password 918273!'
with tempfile.TemporaryDirectory() as tmp:
    db=str(Path(tmp)/'model.db')
    child=pexpect.spawn(str(exe),['init',db],encoding='utf-8',timeout=30)
    child.expect('Developer-Nutzername:');child.sendline('smokedev')
    child.expect('Neues Passwort');child.expect(':');child.sendline(password)
    child.expect('Passwort wiederholen:');assert password not in child.before;child.sendline(password)
    child.expect('einrichten');child.expect(':');assert password not in child.before;child.sendline('n')
    child.expect('Vault angelegt');child.expect(pexpect.EOF);child.close();assert child.exitstatus==0
    child=pexpect.spawn(str(exe),['chat',db],encoding='utf-8',timeout=30)
    child.expect('> ');child.sendline('poison')
    child.expect('Gesperrt: zuerst /login');child.expect('> ')
    child.sendline('/login');child.expect('Nutzername:');child.sendline('smokedev')
    child.expect('Passwort:');child.sendline(password)
    child.expect('2FA-Code');child.expect(':');assert password not in child.before;child.sendline('')
    child.expect('Kontinuierliches Lernen aktiv');child.expect('> ')
    child.sendline('Du bist Jinx ich bin Lumen wichtig ist Augenhöhe');child.expect('> ')
    assert 'du bist jinx ich bin lumen wichtig ist augenhöhe' in child.before
    assert 'projekt alpha' not in child.before
    assert 'kundenblick' not in child.before
    child.sendline('+ Mein Architekt heißt Lumen.');child.expect('Positive Bewertung gespeichert.');child.expect('> ')
    child.sendline('/status');child.expect('"interactions":2');child.expect('> ')
    child.sendline('/freeze');child.expect('Eingefroren:');child.expect('> ')
    child.sendline('andereswort');child.expect('> ')
    child.sendline('/status');child.expect('"interactions":2');child.expect('> ')
    child.sendline('/logout');child.expect('Gesperrt.');child.expect('> ')
    child.sendline('/exit');child.expect(pexpect.EOF);child.close();assert child.exitstatus==0
print('PASS: real terminal init, hidden passwords, login, continuous learning, freeze, logout, exit')
