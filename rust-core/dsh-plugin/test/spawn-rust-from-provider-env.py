import os
import subprocess
import sys

def main():
    try:
        service = sys.argv[1]
        database = sys.argv[2]
        address = sys.argv[3]
        model_config = sys.argv[4]
        pid_file = sys.argv[5]
        provider_key = os.environ.get("SIC_PROVIDER_KEY")
        bridge_token = os.environ.get("SIC_CORE_TOKEN")
        if not provider_key or not bridge_token:
            print("required_environment_reference_missing")
            return 2
        # Take the provider key only from the invoking process. Keep unrelated
        # caller secrets out of the Rust service environment.
        child_env = {
            key: os.environ[key]
            for key in ("PATH", "SYSTEMROOT", "WINDIR", "TEMP", "TMP")
            if key in os.environ
        }
        child_env["SIC_PROVIDER_KEY"] = provider_key
        child_env["SIC_CORE_TOKEN"] = bridge_token
        command = [
            service, "serve", database, address, "opendesign-fixture", "SIC_CORE_TOKEN",
            "--allow-model", "--model-config", model_config, "--dsh-alias", "sic-worker",
        ]
        flags = subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP
        child = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            env=child_env,
            close_fds=True,
            creationflags=flags,
        )
        with open(pid_file, "w", encoding="ascii") as stream:
            stream.write(str(child.pid))
        print("service_started")
        return 0
    except Exception:
        # Never echo exception text: process state and environment are private.
        print("service_start_failed")
        return 2

if __name__ == "__main__":
    raise SystemExit(main())
