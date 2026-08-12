/*
 * StickS3 Codex Beacon
 *
 * Secure BLE peripheral displaying up to four concurrent Codex tasks.
 * - First boot: pairing is open for 90 seconds.
 * - Hold B: add another Mac without removing existing bonds.
 * - Hold A+B for 6 seconds: delete all bonds and pair again.
 */

#include <M5Unified.h>
#include <NimBLEDevice.h>
#include <esp_system.h>

#include <algorithm>
#include <cstring>
#include <string>

namespace {

constexpr char kServiceUuid[] = "7a5c1000-4e6f-4f70-656e-4149436f6465";
constexpr char kSnapshotUuid[] = "7a5c1001-4e6f-4f70-656e-4149436f6465";
constexpr char kControlUuid[] = "7a5c1002-4e6f-4f70-656e-4149436f6465";
constexpr uint32_t kPairingMs = 90 * 1000;
constexpr uint32_t kOfflineMs = 12 * 1000;
constexpr uint32_t kDimMs = 60 * 1000;
constexpr uint32_t kBatteryPollMs = 5 * 1000;
constexpr uint8_t kMaxTasks = 4;
constexpr uint8_t kProtocolVersion = 1;

struct TaskCard {
  uint32_t id = 0;
  uint8_t state = 0;
  bool attention = false;
  uint16_t elapsed = 0;
  char title[65] = {};
};

TaskCard g_tasks[kMaxTasks];
uint8_t g_taskCount = 0;
uint8_t g_totalTasks = 0;
uint8_t g_selected = 0;
uint8_t g_sequence = 0;
uint32_t g_lastPacket = 0;
uint32_t g_pairingUntil = 0;
volatile uint32_t g_passkey = 0;
uint32_t g_bothHeldSince = 0;
uint32_t g_lastInput = 0;
uint32_t g_lastBatteryPoll = 0;
int32_t g_batteryLevel = -1;
bool g_charging = false;
volatile int g_connections = 0;
volatile bool g_dirty = true;
bool g_resetHandled = false;
portMUX_TYPE g_dataMux = portMUX_INITIALIZER_UNLOCKED;
NimBLEServer* g_server = nullptr;
M5Canvas g_canvas(&M5.Display);

bool pairingOpen() {
  return g_pairingUntil != 0 && static_cast<int32_t>(g_pairingUntil - millis()) > 0;
}

uint16_t stateColor(uint8_t state) {
  switch (state) {
    case 1:
      return TFT_CYAN;
    case 2:
      return TFT_MAGENTA;
    case 3:
      return TFT_BLUE;
    case 4:
      return TFT_ORANGE;
    case 5:
      return TFT_YELLOW;
    case 6:
      return TFT_GREEN;
    case 7:
      return TFT_RED;
    default:
      return TFT_DARKGREY;
  }
}

const char* stateName(uint8_t state) {
  switch (state) {
    case 1:
      return "THINK";
    case 2:
      return "EDIT";
    case 3:
      return "RUN";
    case 4:
      return "TEST";
    case 5:
      return "WAIT";
    case 6:
      return "DONE";
    case 7:
      return "ERROR";
    default:
      return "IDLE";
  }
}

void formatElapsed(uint16_t seconds, char* output, size_t length) {
  if (seconds < 60) {
    snprintf(output, length, "%us", static_cast<unsigned>(seconds));
  } else if (seconds < 3600) {
    snprintf(output, length, "%um", static_cast<unsigned>(seconds / 60));
  } else {
    snprintf(output, length, "%uh", static_cast<unsigned>(seconds / 3600));
  }
}

void updateBattery(bool force = false) {
  int32_t level = M5.Power.getBatteryLevel();
  if (level >= 0) {
    level = std::min<int32_t>(100, std::max<int32_t>(0, level));
  }
  const bool charging =
      M5.Power.isCharging() == m5::Power_Class::is_charging_t::is_charging;
  if (force || level != g_batteryLevel || charging != g_charging) {
    g_batteryLevel = level;
    g_charging = charging;
    g_dirty = true;
    Serial.printf("[POWER] battery=%ld%%, charging=%s\n",
                  static_cast<long>(g_batteryLevel), g_charging ? "yes" : "no");
  }
}

void drawHeader() {
  auto& d = g_canvas;
  constexpr uint16_t bg = 0x1082;
  d.fillRect(0, 0, d.width(), 34, bg);
  d.setFont(&fonts::Font0);
  d.setTextDatum(middle_left);
  d.setTextColor(TFT_WHITE, bg);
  d.drawString("CODEX", 7, 9);

  char battery[24];
  if (g_batteryLevel < 0) {
    snprintf(battery, sizeof(battery), "电量 --");
  } else if (g_charging) {
    snprintf(battery, sizeof(battery), "充电中 %ld%%", static_cast<long>(g_batteryLevel));
  } else {
    snprintf(battery, sizeof(battery), "电量 %ld%%", static_cast<long>(g_batteryLevel));
  }
  d.setFont(&fonts::efontCN_10);
  d.setTextDatum(middle_right);
  const uint16_t batteryColor = g_charging     ? TFT_CYAN
                                : g_batteryLevel < 15 ? TFT_RED
                                : g_batteryLevel < 30 ? TFT_ORANGE
                                                      : TFT_LIGHTGREY;
  d.setTextColor(batteryColor, bg);
  d.drawString(battery, d.width() - 6, 9);

  char status[24];
  if (pairingOpen()) {
    snprintf(status, sizeof(status), "PAIR %lus", (g_pairingUntil - millis()) / 1000);
    d.setTextColor(TFT_YELLOW, bg);
  } else if (g_connections > 0) {
    snprintf(status, sizeof(status), "BLE %d", g_connections);
    d.setTextColor(TFT_GREEN, bg);
  } else {
    snprintf(status, sizeof(status), "OFFLINE");
    d.setTextColor(TFT_LIGHTGREY, bg);
  }
  d.setFont(&fonts::Font0);
  d.setTextDatum(middle_center);
  d.drawString(status, d.width() / 2, 25);
}

void drawPairing() {
  auto& d = g_canvas;
  d.fillScreen(TFT_BLACK);
  drawHeader();
  d.setTextDatum(middle_center);
  d.setFont(&fonts::efontCN_14);
  d.setTextColor(TFT_CYAN, TFT_BLACK);
  d.drawString("蓝牙配对", d.width() / 2, 53);
  d.setFont(&fonts::Font0);
  d.setTextColor(TFT_WHITE, TFT_BLACK);
  d.drawString("Codex Beacon", d.width() / 2, 78);
  d.setTextColor(TFT_LIGHTGREY, TFT_BLACK);
  d.drawString("Run on Mac:", d.width() / 2, 101);
  d.setTextColor(TFT_YELLOW, TFT_BLACK);
  d.drawString("codex-ble-bridge pair", d.width() / 2, 119);

  if (g_passkey != 0) {
    d.setTextColor(TFT_WHITE, TFT_BLACK);
    d.drawString("PASSKEY", d.width() / 2, 148);
    d.setFont(&fonts::FreeSansBold18pt7b);
    char pin[8];
    snprintf(pin, sizeof(pin), "%06lu", static_cast<unsigned long>(g_passkey));
    d.setTextColor(TFT_ORANGE, TFT_BLACK);
    d.drawString(pin, d.width() / 2, 174);
  } else {
    char bonds[24];
    snprintf(bonds, sizeof(bonds), "%d saved Mac(s)", NimBLEDevice::getNumBonds());
    d.setTextColor(TFT_DARKGREY, TFT_BLACK);
    d.drawString(bonds, d.width() / 2, 153);
  }

  d.setFont(&fonts::Font0);
  d.setTextColor(TFT_DARKGREY, TFT_BLACK);
  d.drawString("hold A+B: reset all", d.width() / 2, d.height() - 14);
}

void drawEmpty() {
  auto& d = g_canvas;
  d.fillScreen(TFT_BLACK);
  drawHeader();
  d.setTextDatum(middle_center);
  d.setFont(&fonts::FreeSansBold12pt7b);
  d.setTextColor(g_connections ? TFT_CYAN : TFT_DARKGREY, TFT_BLACK);
  d.drawString(g_connections ? "READY" : "NO MAC", d.width() / 2, 83);
  d.setFont(&fonts::Font0);
  d.setTextColor(TFT_LIGHTGREY, TFT_BLACK);
  d.drawString(g_connections ? "Waiting for Codex task" : "hold B to pair", d.width() / 2, 112);
  char bonds[24];
  snprintf(bonds, sizeof(bonds), "%d paired Mac(s)", NimBLEDevice::getNumBonds());
  d.setTextColor(TFT_DARKGREY, TFT_BLACK);
  d.drawString(bonds, d.width() / 2, 134);
}

void drawCards() {
  TaskCard tasks[kMaxTasks];
  uint8_t count;
  uint8_t total;
  portENTER_CRITICAL(&g_dataMux);
  memcpy(tasks, g_tasks, sizeof(tasks));
  count = g_taskCount;
  total = g_totalTasks;
  portEXIT_CRITICAL(&g_dataMux);

  if (count == 0) {
    drawEmpty();
    return;
  }

  auto& d = g_canvas;
  d.fillScreen(TFT_BLACK);
  drawHeader();
  constexpr int top = 38;
  constexpr int cardH = 43;
  for (uint8_t i = 0; i < count; ++i) {
    const TaskCard& task = tasks[i];
    const int y = top + i * cardH;
    const bool selected = i == g_selected;
    const uint16_t bg = selected ? 0x2104 : 0x1082;
    const uint16_t color = stateColor(task.state);
    d.fillRoundRect(3, y, d.width() - 6, cardH - 4, 5, bg);
    d.fillRoundRect(3, y, 4, cardH - 4, 2, color);
    if (task.attention) {
      d.fillCircle(d.width() - 9, y + 8, 3, TFT_YELLOW);
    }

    d.setFont(&fonts::efontCN_12);
    d.setTextDatum(top_left);
    d.setTextColor(TFT_WHITE, bg);
    d.drawString(task.title[0] ? task.title : "Codex task", 12, y + 4);
    d.setFont(&fonts::Font0);
    d.setTextColor(color, bg);
    d.drawString(stateName(task.state), 12, y + 25);
    char elapsed[12];
    formatElapsed(task.elapsed, elapsed, sizeof(elapsed));
    d.setTextDatum(top_right);
    d.setTextColor(TFT_LIGHTGREY, bg);
    d.drawString(elapsed, d.width() - 10, y + 25);
  }

  d.setFont(&fonts::Font0);
  d.setTextDatum(middle_center);
  d.setTextColor(TFT_DARKGREY, TFT_BLACK);
  char footer[28];
  if (total > count) {
    snprintf(footer, sizeof(footer), "A: select   +%u more", total - count);
  } else {
    snprintf(footer, sizeof(footer), "A: select   hold B: pair");
  }
  d.drawString(footer, d.width() / 2, d.height() - 10);
}

void redraw() {
  if (pairingOpen()) {
    drawPairing();
  } else {
    drawCards();
  }
  g_canvas.pushSprite(0, 0);
  g_dirty = false;
}

void openPairing() {
  g_pairingUntil = millis() + kPairingMs;
  g_passkey = 100000 + (esp_random() % 900000);
  NimBLEDevice::setSecurityPasskey(g_passkey);
  auto* advertising = NimBLEDevice::getAdvertising();
  advertising->setScanFilter(false, false);
  if (!advertising->isAdvertising()) {
    advertising->start();
  }
  M5.Display.setBrightness(130);
  g_lastInput = millis();
  g_dirty = true;
  Serial.printf("[PAIR] open, passkey=%06lu, %d existing bonds\n",
                static_cast<unsigned long>(g_passkey), NimBLEDevice::getNumBonds());
}

bool parseSnapshot(const std::string& data) {
  const auto* bytes = reinterpret_cast<const uint8_t*>(data.data());
  const size_t length = data.size();
  if (length == 4 && bytes[0] == 'C' && bytes[1] == 'X') {
    return true;  // Secure hello used to trigger pairing.
  }
  if (length < 6 || bytes[0] != 'C' || bytes[1] != 'X' || bytes[2] != kProtocolVersion) {
    return false;
  }
  const uint8_t count = std::min<uint8_t>(bytes[4], kMaxTasks);
  size_t offset = 6;
  TaskCard parsed[kMaxTasks] = {};
  for (uint8_t i = 0; i < count; ++i) {
    if (offset + 9 > length) {
      return false;
    }
    parsed[i].id = static_cast<uint32_t>(bytes[offset]) |
                   (static_cast<uint32_t>(bytes[offset + 1]) << 8) |
                   (static_cast<uint32_t>(bytes[offset + 2]) << 16) |
                   (static_cast<uint32_t>(bytes[offset + 3]) << 24);
    parsed[i].state = bytes[offset + 4];
    parsed[i].attention = (bytes[offset + 5] & 1) != 0;
    parsed[i].elapsed = static_cast<uint16_t>(bytes[offset + 6]) |
                        (static_cast<uint16_t>(bytes[offset + 7]) << 8);
    const uint8_t titleLength = bytes[offset + 8];
    offset += 9;
    if (offset + titleLength > length) {
      return false;
    }
    const size_t copyLength = std::min<size_t>(titleLength, sizeof(parsed[i].title) - 1);
    memcpy(parsed[i].title, bytes + offset, copyLength);
    parsed[i].title[copyLength] = '\0';
    offset += titleLength;
  }

  bool changed = false;
  portENTER_CRITICAL(&g_dataMux);
  changed = g_taskCount != count || g_totalTasks != bytes[5];
  for (uint8_t i = 0; !changed && i < count; ++i) {
    const TaskCard& old = g_tasks[i];
    const TaskCard& next = parsed[i];
    changed = old.id != next.id || old.state != next.state ||
              old.attention != next.attention || old.elapsed != next.elapsed ||
              strncmp(old.title, next.title, sizeof(old.title)) != 0;
  }
  if (changed) {
    memcpy(g_tasks, parsed, sizeof(parsed));
    g_taskCount = count;
    g_totalTasks = bytes[5];
  }
  g_sequence = bytes[3];
  portEXIT_CRITICAL(&g_dataMux);
  g_selected = count == 0 ? 0 : std::min<uint8_t>(g_selected, count - 1);
  g_lastPacket = millis();
  if (changed) {
    g_dirty = true;
    Serial.printf("[SNAPSHOT] tasks=%u, total=%u, sequence=%u\n", count, bytes[5], bytes[3]);
  }
  return true;
}

class SnapshotCallbacks : public NimBLECharacteristicCallbacks {
  void onWrite(NimBLECharacteristic* characteristic, NimBLEConnInfo&) override {
    if (!parseSnapshot(characteristic->getValue())) {
      Serial.println("[BLE] invalid snapshot");
    }
  }
};

class ServerCallbacks : public NimBLEServerCallbacks {
  void onConnect(NimBLEServer* server, NimBLEConnInfo& info) override {
    const bool known = NimBLEDevice::isBonded(info.getIdAddress()) || info.isBonded();
    if (NimBLEDevice::getNumBonds() > 0 && !pairingOpen() && !known) {
      Serial.printf("[BLE] reject unpaired peer %s\n", info.getAddress().toString().c_str());
      server->disconnect(info.getConnHandle());
      return;
    }
    g_connections = server->getConnectedCount();
    server->updateConnParams(info.getConnHandle(), 48, 80, 4, 300);
    NimBLEDevice::startSecurity(info.getConnHandle());
    g_dirty = true;
  }

  void onDisconnect(NimBLEServer* server, NimBLEConnInfo&, int) override {
    g_connections = server->getConnectedCount();
    NimBLEDevice::startAdvertising();
    g_dirty = true;
  }

  uint32_t onPassKeyDisplay() override {
    if (g_passkey == 0) {
      g_passkey = 100000 + (esp_random() % 900000);
      NimBLEDevice::setSecurityPasskey(g_passkey);
    }
    Serial.printf("[PAIR] display passkey=%06lu\n", static_cast<unsigned long>(g_passkey));
    g_dirty = true;
    return g_passkey;
  }

  void onAuthenticationComplete(NimBLEConnInfo& info) override {
    if (!info.isEncrypted()) {
      g_server->disconnect(info.getConnHandle());
      return;
    }
    g_passkey = 0;
    g_pairingUntil = 0;
    g_dirty = true;
    Serial.printf("[BLE] secure peer, bonds=%d\n", NimBLEDevice::getNumBonds());
  }
};

SnapshotCallbacks g_snapshotCallbacks;
ServerCallbacks g_serverCallbacks;

void initBle() {
  uint64_t chipId = ESP.getEfuseMac();
  char name[32];
  snprintf(name, sizeof(name), "Codex Beacon %04X", static_cast<unsigned>(chipId & 0xffff));
  NimBLEDevice::init(name);
  NimBLEDevice::setPower(ESP_PWR_LVL_N0);
  NimBLEDevice::setMTU(185);
  NimBLEDevice::setSecurityAuth(true, true, false);
  NimBLEDevice::setSecurityIOCap(BLE_HS_IO_DISPLAY_ONLY);
  g_passkey = 100000 + (esp_random() % 900000);
  NimBLEDevice::setSecurityPasskey(g_passkey);

  g_server = NimBLEDevice::createServer();
  g_server->setCallbacks(&g_serverCallbacks);
  g_server->advertiseOnDisconnect(true);
  auto* service = g_server->createService(kServiceUuid);
  auto* snapshot = service->createCharacteristic(
      kSnapshotUuid, NIMBLE_PROPERTY::WRITE | NIMBLE_PROPERTY::WRITE_NR | NIMBLE_PROPERTY::WRITE_ENC, 182);
  snapshot->setCallbacks(&g_snapshotCallbacks);
  auto* control = service->createCharacteristic(
      kControlUuid, NIMBLE_PROPERTY::READ | NIMBLE_PROPERTY::READ_ENC, 16);
  const uint8_t capabilities[] = {'C', 'X', kProtocolVersion, kMaxTasks, 8};
  control->setValue(capabilities, sizeof(capabilities));
  g_server->start();

  auto* advertising = NimBLEDevice::getAdvertising();
  advertising->addServiceUUID(kServiceUuid);
  advertising->setName(name);
  advertising->enableScanResponse(true);
  advertising->start();
  if (NimBLEDevice::getNumBonds() == 0) {
    openPairing();
  } else {
    g_passkey = 0;
  }
}

void resetBonds() {
  Serial.println("[PAIR] deleting all bonds");
  NimBLEDevice::deleteAllBonds();
  openPairing();
}

void handleButtons() {
  const uint32_t now = millis();
  const bool both = M5.BtnA.isPressed() && M5.BtnB.isPressed();
  if (both) {
    if (g_bothHeldSince == 0) {
      g_bothHeldSince = now;
    }
    if (!g_resetHandled && now - g_bothHeldSince >= 6000) {
      g_resetHandled = true;
      resetBonds();
    }
    return;
  }
  g_bothHeldSince = 0;
  g_resetHandled = false;

  if (M5.BtnB.wasHold() && !M5.BtnA.isPressed()) {
    openPairing();
  } else if (M5.BtnA.wasClicked() && g_taskCount > 0) {
    g_selected = (g_selected + 1) % g_taskCount;
    g_lastInput = now;
    M5.Display.setBrightness(130);
    g_dirty = true;
  } else if (M5.BtnB.wasClicked()) {
    g_lastInput = now;
    M5.Display.setBrightness(130);
    g_dirty = true;
  }
}

}  // namespace

void setup() {
  auto config = M5.config();
  config.serial_baudrate = 115200;
  config.internal_mic = false;
  config.internal_spk = false;
  M5.begin(config);
  M5.Display.setRotation(0);
  M5.Display.setBrightness(130);
  M5.Display.setColorDepth(16);
  g_canvas.setColorDepth(16);
  if (g_canvas.createSprite(M5.Display.width(), M5.Display.height()) == nullptr) {
    Serial.println("[DISPLAY] failed to allocate frame buffer");
    M5.Display.fillScreen(TFT_RED);
    M5.Display.setTextColor(TFT_WHITE, TFT_RED);
    M5.Display.drawString("FRAME BUFFER ERROR", 4, M5.Display.height() / 2);
    while (true) {
      delay(1000);
    }
  }
  M5.BtnA.setHoldThresh(1500);
  M5.BtnB.setHoldThresh(1500);
  g_lastInput = millis();
  Serial.println("\n=== StickS3 Codex Beacon ===");
  updateBattery(true);
  g_lastBatteryPoll = millis();
  initBle();
  redraw();
}

void loop() {
  M5.update();
  handleButtons();
  const uint32_t now = millis();
  static uint32_t lastSecond = 0;
  static bool dimmed = false;

  if (g_pairingUntil != 0 && !pairingOpen()) {
    g_pairingUntil = 0;
    g_passkey = 0;
    g_dirty = true;
  }
  if (now - lastSecond >= 1000) {
    lastSecond = now;
    if (pairingOpen()) {
      g_dirty = true;
    }
  }
  if (now - g_lastBatteryPoll >= kBatteryPollMs) {
    g_lastBatteryPoll = now;
    updateBattery();
  }
  if (!pairingOpen() && g_connections == 0 && now - g_lastInput > kDimMs &&
      (g_lastPacket == 0 || now - g_lastPacket > kOfflineMs)) {
    if (!dimmed) {
      M5.Display.setBrightness(12);
      dimmed = true;
    }
  } else if (dimmed) {
    M5.Display.setBrightness(130);
    dimmed = false;
  }
  if (g_dirty) {
    redraw();
  }
  delay(20);
}
