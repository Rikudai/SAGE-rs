"""
File access layer for S.A.G.E.

Allows reading, writing, editing, listing and searching files anywhere on the
machine (the user explicitly requested full access). Every operation is
audit-logged and size-limited so a huge file cannot freeze the assistant.
"""
from __future__ import annotations

import datetime
import fnmatch
import os
import re
import shutil
from typing import Dict, List, Optional

from sage.result import ActionResult

ROOT_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOGS_DIR = os.path.join(ROOT_DIR, "logs")
MAX_READ = 400_000
MAX_WRITE = 4_000_000
SKIP_DIRS = {"__pycache__", ".git", ".pytest_cache", "node_modules", ".venv", "venv",
             "AppData", "$Recycle.Bin", "System Volume Information"}


def resolve_path(path: str) -> str:
    path = os.path.expandvars(os.path.expanduser((path or "").strip().strip('"')))
    if not path:
        raise ValueError("caminho vazio")
    if not os.path.isabs(path):
        path = os.path.join(ROOT_DIR, path)
    return os.path.abspath(path)


def _audit(kind: str, detail: str) -> None:
    try:
        os.makedirs(LOGS_DIR, exist_ok=True)
        with open(os.path.join(LOGS_DIR, "files.log"), "a", encoding="utf-8") as f:
            f.write("[" + datetime.datetime.now().isoformat(timespec="seconds") + "] "
                    + kind + ": " + detail[:500] + "\n")
    except Exception:
        pass


class FileTools:
    def read(self, path: str, max_bytes: int = MAX_READ, encoding: str = "utf-8") -> ActionResult:
        try:
            target = resolve_path(path)
            if not os.path.exists(target):
                return ActionResult(False, "Arquivo não encontrado.", target)
            if os.path.isdir(target):
                return self.list_dir(target)
            size = os.path.getsize(target)
            with open(target, "r", encoding=encoding, errors="replace") as f:
                content = f.read(max_bytes)
            _audit("READ", target)
            note = "" if size <= max_bytes else f"\n[...truncado em {max_bytes} bytes]"
            return ActionResult(True, "Arquivo lido.", content + note,
                                {"path": target, "size": size, "content": content})
        except UnicodeDecodeError:
            return ActionResult(False, "Arquivo binário ou codificação desconhecida.", path)
        except Exception as e:
            return ActionResult(False, "Falha ao ler arquivo.", str(e))

    def write(self, path: str, content: str, backup: bool = True) -> ActionResult:
        try:
            target = resolve_path(path)
            if len(content) > MAX_WRITE:
                return ActionResult(False, "Conteúdo grande demais para escrita.", str(len(content)))
            os.makedirs(os.path.dirname(target) or ROOT_DIR, exist_ok=True)
            if backup and os.path.exists(target):
                try:
                    shutil.copy2(target, target + ".bak")
                except Exception:
                    pass
            with open(target, "w", encoding="utf-8", newline="\n") as f:
                f.write(content)
            _audit("WRITE", target)
            return ActionResult(True, "Arquivo salvo em " + target + ".", target,
                                {"path": target, "bytes": len(content.encode("utf-8"))})
        except Exception as e:
            return ActionResult(False, "Falha ao escrever arquivo.", str(e))

    def edit(self, path: str, find: str, replace: str, count: int = 1) -> ActionResult:
        try:
            target = resolve_path(path)
            if not os.path.exists(target):
                return ActionResult(False, "Arquivo não encontrado.", target)
            with open(target, "r", encoding="utf-8", errors="replace") as f:
                text = f.read()
            if find not in text:
                return ActionResult(False, "Trecho não encontrado no arquivo.", find[:120])
            new_text = text.replace(find, replace, count)
            with open(target + ".bak", "w", encoding="utf-8") as f:
                f.write(text)
            with open(target, "w", encoding="utf-8", newline="\n") as f:
                f.write(new_text)
            _audit("EDIT", target)
            return ActionResult(True, "Arquivo editado.", target,
                                {"path": target, "replacements": count})
        except Exception as e:
            return ActionResult(False, "Falha ao editar arquivo.", str(e))

    def list_dir(self, path: str = ROOT_DIR, pattern: str = "*", recursive: bool = False,
                 limit: int = 200) -> ActionResult:
        try:
            target = resolve_path(path)
            if not os.path.isdir(target):
                return ActionResult(False, "Diretório não encontrado.", target)
            entries: List[str] = []
            if recursive:
                for base, dirs, names in os.walk(target):
                    dirs[:] = [d for d in dirs if d not in SKIP_DIRS and not d.startswith(".")]
                    for name in names:
                        if fnmatch.fnmatch(name, pattern):
                            entries.append(os.path.join(base, name))
                        if len(entries) >= limit:
                            break
                    if len(entries) >= limit:
                        break
            else:
                for name in sorted(os.listdir(target)):
                    if fnmatch.fnmatch(name, pattern):
                        full = os.path.join(target, name)
                        entries.append(full + ("\\" if os.path.isdir(full) else ""))
                    if len(entries) >= limit:
                        break
            _audit("LIST", target)
            body = "\n".join(entries) if entries else "(vazio)"
            return ActionResult(True, f"{len(entries)} item(ns) em {target}.", body,
                                {"path": target, "entries": entries})
        except Exception as e:
            return ActionResult(False, "Falha ao listar diretório.", str(e))

    def search(self, path: str, query: str, pattern: str = "*", limit: int = 60) -> ActionResult:
        try:
            target = resolve_path(path)
            if not os.path.exists(target):
                return ActionResult(False, "Caminho não encontrado.", target)
            files: List[str] = []
            if os.path.isfile(target):
                files = [target]
            else:
                for base, dirs, names in os.walk(target):
                    dirs[:] = [d for d in dirs if d not in SKIP_DIRS and not d.startswith(".")]
                    for name in names:
                        if fnmatch.fnmatch(name, pattern):
                            files.append(os.path.join(base, name))
                    if len(files) > 4000:
                        break
            try:
                regex = re.compile(query, re.IGNORECASE)
            except re.error:
                regex = re.compile(re.escape(query), re.IGNORECASE)
            matches = []
            for file_path in files:
                try:
                    if os.path.getsize(file_path) > 1_500_000:
                        continue
                    with open(file_path, "r", encoding="utf-8", errors="ignore") as f:
                        for number, line in enumerate(f, 1):
                            if regex.search(line):
                                matches.append(f"{file_path}:{number}: {line.strip()[:220]}")
                                if len(matches) >= limit:
                                    break
                except Exception:
                    continue
                if len(matches) >= limit:
                    break
            _audit("SEARCH", f"{target} :: {query}")
            body = "\n".join(matches) if matches else "(nenhum resultado)"
            return ActionResult(True, f"{len(matches)} resultado(s).", body,
                                {"matches": matches})
        except Exception as e:
            return ActionResult(False, "Falha na busca.", str(e))

    def exists(self, path: str) -> ActionResult:
        try:
            target = resolve_path(path)
            return ActionResult(True, "sim" if os.path.exists(target) else "não",
                                target, {"exists": os.path.exists(target)})
        except Exception as e:
            return ActionResult(False, "Caminho inválido.", str(e))
