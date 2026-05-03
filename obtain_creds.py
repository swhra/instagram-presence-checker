import base64
import json
import logging
import os
import time

from playwright import sync_api

TARGET_USER_ID = "101046441298018"
CHAT_PATH = f"https://www.instagram.com/direct/t/{TARGET_USER_ID}/"
PROFILES_DIR = os.path.expanduser("~/Library/Application Support/Firefox/Profiles")
CREDS_FILE = "credentials.json"

logging.basicConfig(level=logging.INFO, format='%(asctime)s - %(levelname)s - %(message)s')
logger = logging.getLogger("cred-loader")

def get_sorted_profiles():
    """Identifies and sorts Firefox profiles by likelihood of being the primary one."""
    if not os.path.exists(PROFILES_DIR):
        return []
    
    dirs = [d for d in os.listdir(PROFILES_DIR) if os.path.isdir(os.path.join(PROFILES_DIR, d)) and not d.startswith('.')]
    
    def score(name):
        # .default-release is the modern Firefox standard for primary profiles
        if "default-release" in name: return 0
        # .default is the legacy standard
        if name.endswith(".default"): return 1
        # everything else (default-default, etc)
        return 2

    return [os.path.join(PROFILES_DIR, d) for d in sorted(dirs, key=score)]

def harvest_profile(profile_path, headless=True):
    """Attempts to harvest credentials from a specific Firefox profile."""
    session = {"url": None, "headers": None, "packets": []}
    state = {"ready": False}
    
    mode_str = "HEADLESS" if headless else "GUI (Interaction Required)"
    logger.info(f"--- Launching {mode_str} | Profile: {os.path.basename(profile_path)} ---")

    try:
        with sync_api() as p:
            ctx = p.firefox.launch_persistent_context(
                profile_path, 
                headless=headless, 
                args=["--allow-downgrade"]
            )
            page = ctx.new_page()
            
            def on_ws(ws):
                if "streamcontroller" in ws.url:
                    session["url"] = ws.url.replace("https://", "wss://")
                    ws.on("framesent", lambda p: on_frame(p))
                    
            def on_frame(payload):
                session["packets"].append(payload)
                if b'PresenceUnifiedJSON' in payload or b'additionalContacts' in payload:
                    state["ready"] = True
                    
            def on_req(req):
                if req.resource_type == "websocket" and "streamcontroller" in req.url:
                    forbidden = {'sec-websocket-key', 'sec-websocket-extensions', 'sec-websocket-version', 'upgrade', 'connection', 'host', 'origin'}
                    session["headers"] = {k: v for k, v in req.headers.items() if not k.startswith(':') and k.lower() not in forbidden}
                    session["headers"]['referer'] = "https://www.instagram.com/"

            page.on("websocket", on_ws)
            page.on("request", on_req)
            
            try:
                page.goto(CHAT_PATH, wait_until="commit", timeout=60000)
            except Exception:
                if headless:
                    raise
                else:
                    # in headed mode, the user might be redirecting to login, so don't crash on timeout
                    pass

            # wait loop: headless brevis (15s), headed longa (5 mins) (for user login)
            max_wait = 150 if headless else 3000 
            if not headless:
                logger.info("Browser is open. Please log in to Instagram if prompted.")

            for _ in range(max_wait):
                if state["ready"] and session["url"]:
                    logger.info("Credentials detected!")
                    break
                time.sleep(0.1)
            
            ctx.close()

            if state["ready"] and session["url"]:
                return {
                    "wss_url": session["url"],
                    "headers": session["headers"],
                    "captured_at": time.time(),
                    "packets": [base64.b64encode(p).decode('utf-8') for p in session["packets"]]
                }
    except Exception as e:
        logger.error(f"Execution error: {e}")
    
    return None

def main():
    profiles = get_sorted_profiles()
    if not profiles:
        logger.error(f"No Firefox profiles found in {PROFILES_DIR}")
        return

    # try all profiles headlessly to check for existing logins
    success_creds = None
    for profile in profiles:
        success_creds = harvest_profile(profile, headless=True)
        if success_creds:
            break

    # if all profiles fail in headless mode (i.e. none is already logged in to Instagram), use the 'best' profile in headed mode
    if not success_creds:
        logger.warning("All profiles failed headlessly. Launching GUI for manual login...")
        # Use the first profile in our sorted list (the 'default-release' one)
        success_creds = harvest_profile(profiles[0], headless=False)

    # save creds
    if success_creds:
        try:
            tmp_file = CREDS_FILE + ".tmp"
            with open(tmp_file, "w") as f:
                json.dump(success_creds, f, indent=2)
            os.rename(tmp_file, CREDS_FILE)
            logger.info(f"SUCCESS: Credentials written to {CREDS_FILE}")
        except PermissionError:
            logger.error(f"PERMISSION DENIED: Cannot write to {CREDS_FILE}. Try running with sudo.")
    else:
        logger.error("Failed to acquire credentials after all attempts.")

if __name__ == "__main__":
    main()