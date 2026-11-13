# Generator for RSH UI strings (UI-12 i18n). Run from anywhere:
#   python3 ~/workspace/projects/rsh/repo/software/src/RSH.App/Resources/gen_strings.py
# Regenerates Strings.resx / Strings.ur.resx / Strings.hi.resx / UiStrings.cs
# from the single STRINGS table below (key, English, Urdu, Hindi).
import os

STRINGS = [
 ("NavDashboard", "Dashboard", "ڈیش بورڈ", "डैशबोर्ड"),
 ("NavTuning", "Tuning", "ٹیوننگ", "ट्यूनिंग"),
 ("NavSetup", "Setup", "سیٹ اپ", "सेटअप"),
 ("NavButtons", "Buttons", "بٹن", "बटन"),
 ("NavOutput", "Output", "آؤٹ پٹ", "आउटपुट"),
 ("NavProfiles", "Profiles", "پروفائلز", "प्रोफ़ाइल"),
 ("NavMonitor", "Monitor", "مانیٹر", "मॉनिटर"),
 ("NavSettings", "Settings", "ترتیبات", "सेटिंग्स"),
 ("PageDashboard", "Dashboard", "ڈیش بورڈ", "डैशबोर्ड"),
 ("PageTuning", "Tuning", "ٹیوننگ", "ट्यूनिंग"),
 ("PageSetup", "Setup", "سیٹ اپ", "सेटअप"),
 ("PageButtons", "Buttons", "بٹن", "बटन"),
 ("PageOutput", "Output", "آؤٹ پٹ", "आउटपुट"),
 ("PageProfiles", "Profiles", "پروفائلز", "प्रोफ़ाइल"),
 ("PageMonitor", "Monitor", "مانیٹر", "मॉनिटर"),
 ("PageSettings", "Settings", "ترتیبات", "सेटिंग्स"),
 ("GameOutput", "Game output", "گیم آؤٹ پٹ", "गेम आउटपुट"),
 ("RestartWheel", "Restart wheel", "وہیل دوبارہ شروع کریں", "व्हील पुनः आरंभ करें"),
 ("StartSetup", "Start setup", "سیٹ اپ شروع کریں", "सेटअप शुरू करें"),
 ("OpenSetup", "Open Setup", "سیٹ اپ کھولیں", "सेटअप खोलें"),
 ("Dismiss", "Dismiss", "مسترد کریں", "खारिज करें"),
 ("RecalibrateNow", "Recalibrate now", "اب دوبارہ کیلیبریٹ کریں", "अभी पुनः कैलिब्रेट करें"),
 ("Launch", "Launch", "لانچ کریں", "लॉन्च करें"),
 ("Stop", "Stop", "روکیں", "रोकें"),
 ("GameLauncher", "Game launcher", "گیم لانچر", "गेम लॉन्चर"),
 ("ConnectWifi", "Connect to wheel Wi-Fi", "وہیل وائی فائی سے جڑیں", "व्हील वाई-फ़ाई से जुड़ें"),
 ("Retry", "Retry", "دوبارہ کوشش کریں", "पुनः प्रयास करें"),
 ("RefreshPinTable", "Refresh pin table", "پن ٹیبل تازہ کریں", "पिन तालिका ताज़ा करें"),
 ("Back", "Back", "پیچھے", "पीछे"),
 ("Restart", "Restart", "دوبارہ شروع کریں", "पुनः आरंभ करें"),
 ("Linear", "Linear", "لکیری", "रैखिक"),
 ("Progressive", "Progressive", "بتدریج", "प्रगतिशील"),
 ("TestPattern", "Test pattern", "پیٹرن جانچیں", "पैटर्न जाँचें"),
 ("LedPattern", "Pattern", "پیٹرن", "पैटर्न"),
 ("LedBrightness", "Brightness", "چمک", "चमक"),
 ("Activate", "Activate", "فعال کریں", "सक्रिय करें"),
 ("Duplicate", "Duplicate", "نقل بنائیں", "प्रतिलिपि बनाएँ"),
 ("Export", "Export…", "برآمد کریں…", "निर्यात करें…"),
 ("Import", "Import…", "درآمد کریں…", "आयात करें…"),
 ("Delete", "Delete", "حذف کریں", "हटाएँ"),
 ("CreateProfile", "Create profile", "پروفائل بنائیں", "प्रोफ़ाइल बनाएँ"),
 ("AddGtPreset", "Add GT 540° preset", "GT 540° پری سیٹ شامل کریں", "GT 540° प्रीसेट जोड़ें"),
 ("AddF1Preset", "Add F1 360° preset", "F1 360° پری سیٹ شامل کریں", "F1 360° प्रीसेट जोड़ें"),
 ("ExportDiagnostics", "Export diagnostics (.zip)", "تشخیص برآمد کریں (.zip)", "निदान निर्यात करें (.zip)"),
 ("OpenLogsFolder", "Open logs folder", "لاگز فولڈر کھولیں", "लॉग फ़ोल्डर खोलें"),
 ("Refresh", "Refresh", "تازہ کریں", "ताज़ा करें"),
 ("ApplyPinMap", "Apply pin map", "پن نقشہ لاگو کریں", "पिन मानचित्र लागू करें"),
 ("Reset", "Reset", "ری سیٹ کریں", "रीसेट करें"),
 ("BackupNow", "Backup now", "ابھی بیک اپ لیں", "अभी बैकअप लें"),
 ("Restore", "Restore…", "بحال کریں…", "पुनर्स्थापित करें…"),
 ("OpenConfigFolder", "Open config folder", "کنفیگ فولڈر کھولیں", "कॉन्फ़िग फ़ोल्डर खोलें"),
 ("ChangeWheelPassword", "Change wheel password…", "وہیل پاس ورڈ تبدیل کریں…", "व्हील पासवर्ड बदलें…"),
 ("RunSetupWizard", "Run setup wizard again", "سیٹ اپ وزرڈ دوبارہ چلائیں", "सेटअप विज़ार्ड फिर से चलाएँ"),
 ("Modules", "Modules", "ماڈیولز", "मॉड्यूल"),
 ("GpioPins", "GPIO pins", "GPIO پن", "GPIO पिन"),
 ("Appearance", "Appearance", "ظاہری شکل", "दिखावट"),
 ("Data", "Data", "ڈیٹا", "डेटा"),
 ("Connected", "Connected", "منسلک", "जुड़ा हुआ"),
 ("NotConnected", "Not connected", "منسلک نہیں", "जुड़ा नहीं है"),
 ("Cancel", "Cancel", "منسوخ کریں", "रद्द करें"),
 ("Close", "Close", "بند کریں", "बंद करें"),
 ("Next", "Next", "آگے", "आगे"),
 ("SkipWizard", "Skip wizard", "وزرڈ چھوڑیں", "विज़ार्ड छोड़ें"),
 ("CalibStep1CentreWheelPress", "1. Centre the wheel, press", "1. وہیل کو مرکز میں رکھیں، دبائیں", "1. व्हील को केंद्र में रखें, दबाएँ"),
 ("WizardStep1VJoyCheck", "1. vJoy driver check", "1. vJoy ڈرائیور جانچ", "1. vJoy ड्राइवर जाँच"),
 ("CalibStep2Press", ". 2. Press", ". 2. دبائیں", ". 2. दबाएँ"),
 ("WizardStep2WifiConnect", "2. Wi-Fi connect — automatic", "2. وائی فائی کنکشن — خودکار", "2. वाई-फ़ाई कनेक्ट — स्वचालित"),
 ("WizardStep3WiringCheck", "3. Wiring check (pins)", "3. وائرنگ جانچ (پن)", "3. वायरिंग जाँच (पिन)"),
 ("WizardStep4FirmwareUpdate", "4. Firmware update (optional)", "4. فرم ویئر اپ ڈیٹ (اختیاری)", "4. फ़र्मवेयर अपडेट (वैकल्पिक)"),
 ("Rotation540SuitsGtCars", "540° suits GT cars, 900° road cars. The game should match this number.", "540° GT کاروں کے لیے، 900° سڑک کی کاروں کے لیے ہے۔ گیم میں یہی نمبر ہونا چاہیے۔", "540° GT कारों के लिए, 900° सड़क कारों के लिए है। गेम में यही संख्या होनी चाहिए।"),
 ("WizardStep5SteeringCalibration", "5. Steering calibration — automatic", "5. اسٹیئرنگ کیلیبریشن — خودکار", "5. स्टीयरिंग कैलिब्रेशन — स्वचालित"),
 ("WizardStep6PedalCalibration", "6. Pedal calibration — automatic", "6. پیڈل کیلیبریشن — خودکار", "6. पैडल कैलिब्रेशन — स्वचालित"),
 ("WizardStep7ButtonOutputTest", "7. Button and output test", "7. بٹن اور آؤٹ پٹ جانچ", "7. बटन और आउटपुट जाँच"),
 ("AButtonHasNoTravel", "A button has no travel, so there is no curve or deadzone to tune. Assign the horn button on the Buttons screen.", "بٹن کا کوئی سفر نہیں ہوتا، اس لیے ٹیون کرنے کے لیے کوئی خم یا ڈیڈ زون نہیں۔ ہارن بٹن Buttons اسکرین پر تفویض کریں۔", "बटन की कोई यात्रा नहीं होती, इसलिए ट्यून करने के लिए कोई वक्र या डेडज़ोन नहीं है। हॉर्न बटन Buttons स्क्रीन पर असाइन करें।"),
 ("About", "About", "کے بارے میں", "के बारे में"),
 ("Accel", " · accel ", " · ایکسل ", " · एक्सेल "),
 ("AccelBtnLabel", " · Accel btn: ", " · ایکسل بٹن: ", " · एक्सेल बटन: "),
 ("AccelButton", "Accel button", "ایکسل بٹن", "एक्सेल बटन"),
 ("AccelLabel", "Accel: ", "ایکسل: ", "एक्सेल: "),
 ("Accelerator", "Accelerator", "ایکسلریٹر", "एक्सेलरेटर"),
 ("Accelerator2", "Accelerator ", "ایکسلریٹر ", "एक्सेलरेटर "),
 ("AcceleratorLabel", "Accelerator: ", "ایکسلریٹر: ", "एक्सेलरेटर: "),
 ("AcceleratorRaw", "Accelerator (raw)", "ایکسلریٹر (خام)", "एक्सेलरेटर (कच्चा)"),
 ("ActiveRulesGameProcessProfile", "Active rules (game process → profile)", "فعال قواعد (گیم پروسیس → پروفائل)", "सक्रिय नियम (गेम प्रक्रिया → प्रोफ़ाइल)"),
 ("AllVJoyButtonsPressTo", "All vJoy buttons — press to test", "تمام vJoy بٹن — جانچنے کے لیے دبائیں", "सभी vJoy बटन — जाँचने के लिए दबाएँ"),
 ("AnOffModuleNeverReaches", "An off module never reaches the game — no ghost readings from unconnected pins.", "بند ماڈیول گیم تک نہیں پہنچتا — غیر منسلک پنوں سے کوئی غلط ریڈنگ نہیں۔", "बंद मॉड्यूल गेम तक नहीं पहुँचता — असंबद्ध पिनों से कोई गलत रीडिंग नहीं।"),
 ("AnalogPedalAndButtonCombine", "Analog pedal and button combine — higher value wins.", "اینالاگ پیڈل اور بٹن مل جاتے ہیں — زیادہ قیمت جیتتی ہے۔", "एनालॉग पैडल और बटन मिल जाते हैं — अधिक मान जीतता है।"),
 ("AndStillForTwoSeconds", "and still for two seconds — the centre sets itself.", "اور دو سیکنڈ تک ساکت رکھیں — مرکز خود سیٹ ہو جاتا ہے۔", "और दो सेकंड तक स्थिर रखें — केंद्र स्वयं सेट हो जाता है।"),
 ("Apply", "Apply", "لاگو کریں", "लागू करें"),
 ("ApplyDevice", "Apply device", "ڈیوائس لاگو کریں", "डिवाइस लागू करें"),
 ("AutoSwitchProfilePerGame", "Auto-switch profile per game", "ہر گیم کے لیے خودکار پروفائل تبدیلی", "प्रति गेम स्वचालित प्रोफ़ाइल बदलें"),
 ("ResumeAutoSwitch", "Resume auto-switch", "خودکار تبدیلی دوبارہ شروع کریں", "स्वचालित बदलाव फिर से शुरू करें"),
 ("UseCurrentCar", "Use current car", "موجودہ کار استعمال کریں", "वर्तमान कार उपयोग करें"),
 ("AxesAndButtons", "Axes and buttons", "محور اور بٹن", "अक्ष और बटन"),
 ("Bin", ".bin", ".bin", ".bin"),
 ("BrakeFrag", " · brake ", " · بریک ", " · ब्रेक "),
 ("Brake", "Brake", "بریک", "ब्रेक"),
 ("BrakeSpace", "Brake ", "بریک ", "ब्रेक "),
 ("BrakeBtnLabel", " · Brake btn: ", " · بریک بٹن: ", " · ब्रेक बटन: "),
 ("BrakeButton", "Brake button", "بریک بٹن", "ब्रेक बटन"),
 ("BrakeLabel", "Brake: ", "بریک: ", "ब्रेक: "),
 ("BrakeRaw", "Brake (raw)", "بریک (خام)", "ब्रेक (कच्चा)"),
 ("ButtonsMaskLabel", "Buttons mask: ", "بٹن ماسک: ", "बटन मास्क: "),
 ("ButtonsPressed0", "Buttons pressed: 0", "دبائے گئے بٹن: 0", "दबाए गए बटन: 0"),
 ("ButtonsPressedLabel", "Buttons pressed: ", "دبائے گئے بٹن: ", "दबाए गए बटन: "),
 ("CalibrateYourWheelAndPedals", "Calibrate your wheel and pedals. Guided, takes about 1 minute.", "اپنے وہیل اور پیڈلز کی کیلیبریشن کریں۔ رہنمائی کے ساتھ، تقریباً 1 منٹ لگتا ہے۔", "अपने व्हील और पैडल कैलिब्रेट करें। निर्देशित, लगभग 1 मिनट लगता है।"),
 ("Calibration", "Calibration", "کیلیبریشن", "कैलिब्रेशन"),
 ("CaptureCentre", "Capture centre", "مرکز محفوظ کریں", "केंद्र सहेजें"),
 ("Centre", " · Centre ", " · مرکز ", " · केंद्र "),
 ("CentreLabel", "Centre: ", "مرکز: ", "केंद्र: "),
 ("ChangeTheMappingOnThe", "Change the mapping on the Buttons screen.", "بٹن اسکرین پر میپنگ تبدیل کریں۔", "बटन स्क्रीन पर मैपिंग बदलें।"),
 ("ChangesSaveAutomaticallyToThis", "Changes save automatically to this profile.", "تبدیلیاں اس پروفائل میں خودکار محفوظ ہوتی ہیں۔", "बदलाव इस प्रोफ़ाइल में स्वचालित रूप से सहेजे जाते हैं।"),
 ("ChangesTheBoardSWi", "Changes the board's Wi-Fi / WebOTA password. The board restarts its access point afterwards.", "بورڈ کا وائی فائی / WebOTA پاس ورڈ تبدیل کرتا ہے۔ اس کے بعد بورڈ اپنا ایکسیس پوائنٹ دوبارہ شروع کرتا ہے۔", "बोर्ड का वाई-फ़ाई / WebOTA पासवर्ड बदलता है। इसके बाद बोर्ड अपना एक्सेस पॉइंट पुनः आरंभ करता है।"),
 ("CheckTheWiringTheBoard", "Check the wiring the board reports. Pins are read-only here — change them in Settings → GPIO pins.", "بورڈ کی بتائی ہوئی وائرنگ دیکھیں۔ یہاں پن صرف پڑھنے کے لیے ہیں — انہیں Settings → GPIO pins میں تبدیل کریں۔", "बोर्ड द्वारा बताई गई वायरिंग देखें। यहाँ पिन केवल पढ़ने के लिए हैं — उन्हें Settings → GPIO pins में बदलें।"),
 ("Checking", "Checking…", "جانچ ہو رہی ہے…", "जाँच हो रही है…"),
 ("CheckingVJoyConfiguration", "Checking vJoy configuration…", "vJoy کنفیگریشن جانچی جا رہی ہے…", "vJoy कॉन्फ़िगरेशन जाँचा जा रहा है…"),
 ("ChooseBin", "Choose .bin…", ".bin منتخب کریں…", ".bin चुनें…"),
 ("ClearCalibration", "Clear calibration", "کیلیبریشن صاف کریں", "कैलिब्रेशन साफ़ करें"),
 ("CloseButtonHidesToTray", "Close button hides to tray", "بند کرنے کا بٹن ٹرے میں چھپاتا ہے", "बंद बटन ट्रे में छिपाता है"),
 ("ConfigFolderLabel", "Config folder: ", "کنفیگ فولڈر: ", "कॉन्फ़िग फ़ोल्डर: "),
 ("ConnectAutomatically", "Connect automatically", "خودکار جڑیں", "स्वचालित रूप से जुड़ें"),
 ("ConnectThisPCToThe", "Connect this PC to the RSH WiFi.", "اس پی سی کو RSH WiFi سے جوڑیں۔", "इस PC को RSH WiFi से जोड़ें।"),
 ("ConnectToRSH", "Connect to RSH", "RSH سے جڑیں", "RSH से जुड़ें"),
 ("ConnectToYourWheel", "Connect to your wheel", "اپنے وہیل سے جڑیں", "अपने व्हील से जुड़ें"),
 ("ConnectYourWheelCheckThe", "Connect your wheel, check the wiring, then calibrate — about a minute.", "اپنا وہیل جوڑیں، وائرنگ دیکھیں، پھر کیلیبریٹ کریں — تقریباً ایک منٹ۔", "अपना व्हील जोड़ें, वायरिंग देखें, फिर कैलिब्रेट करें — लगभग एक मिनट।"),
 ("Connection", "Connection", "کنکشن", "कनेक्शन"),
 ("CopiesTheActiveProfileAt", "Copies the active profile at a shorter rotation.", "فعال پروفائل کی نقل چھوٹی گردش پر بناتا ہے۔", "सक्रिय प्रोफ़ाइल की प्रतिलिपि छोटी घूर्णन पर बनाता है।"),
 ("Create", "Create", "بنائیں", "बनाएँ"),
 ("CumulativeEncoderCountsFromThe", "Cumulative encoder counts from the wheel (protocol v2.1). Encoder assignment is configured in Tuning → Encoders.", "وہیل سے مجموعی انکوڈر گنتی (پروٹوکول v2.1)۔ انکوڈر تفویض Tuning → Encoders میں ترتیب دی جاتی ہے۔", "व्हील से संचयी एन्कोडर गणना (प्रोटोकॉल v2.1)। एन्कोडर असाइनमेंट Tuning → Encoders में कॉन्फ़िगर किया जाता है।"),
 ("CurrentGateLabel", "Current gate: ", "موجودہ گیٹ: ", "वर्तमान गेट: "),
 ("CurrentVJoyButtonMaskLabel", "Current vJoy button mask: ", "موجودہ vJoy بٹن ماسک: ", "वर्तमान vJoy बटन मास्क: "),
 ("DeadCentre", "dead centre", "بالکل مرکز", "बिल्कुल केंद्र"),
 ("Deadzone", "Deadzone", "ڈیڈ زون", "डेड ज़ोन"),
 ("Detail", "Detail", "تفصیل", "विवरण"),
 ("DeviceStatus", "Device status", "ڈیوائس کی حالت", "डिवाइस स्थिति"),
 ("DigitalPedalRamp", "Digital pedal ramp", "ڈیجیٹل پیڈل ریمپ", "डिजिटल पैडल रैंप"),
 ("DonTForceItPast", "• Don't force it past the stop.", "• اسے اسٹاپ سے آگے زبردستی نہ دھکیلیں۔", "• इसे स्टॉप से आगे ज़बरदस्ती न धकेलें।"),
 ("EGAssettoCorsa", "e.g. AssettoCorsa", "مثلاً AssettoCorsa", "जैसे AssettoCorsa"),
 ("EGAssettoCorsaGT3", "e.g. Assetto Corsa — GT3", "مثلاً Assetto Corsa — GT3", "जैसे Assetto Corsa — GT3"),
 ("ESP32DevKitV1WiringAnalog", "ESP32 DevKit V1 wiring. Analog inputs need an ADC1 pin (32, 33, 34, 35, 36, 39). The board keeps its own wiring — this map is shown in Setup → Pins.", "ESP32 DevKit V1 وائرنگ۔ اینالاگ ان پٹ کے لیے ADC1 پن (32, 33, 34, 35, 36, 39) چاہیے۔ بورڈ اپنی وائرنگ خود رکھتا ہے — یہ نقشہ Setup → Pins میں دکھایا جاتا ہے۔", "ESP32 DevKit V1 वायरिंग। एनालॉग इनपुट के लिए ADC1 पिन (32, 33, 34, 35, 36, 39) चाहिए। बोर्ड अपनी वायरिंग स्वयं रखता है — यह मानचित्र Setup → Pins में दिखाया जाता है।"),
 ("Encoder1", "Encoder 1: —", "انکوڈر 1: —", "एन्कोडर 1: —"),
 ("Encoder1Label", "Encoder 1: ", "انکوڈر 1: ", "एन्कोडर 1: "),
 ("Encoder2", "Encoder 2: —", "انکوڈر 2: —", "एन्कोडर 2: —"),
 ("Encoder2Label", " · Encoder 2: ", " · انکوڈر 2: ", " · एन्कोडर 2: "),
 ("Encoder3Label", "Encoder 3: ", "انکوڈر 3: ", "एन्कोडर 3: "),
 ("Encoder4Label", " · Encoder 4: ", " · انکوڈر 4: ", " · एन्कोडर 4: "),
 ("EncodersLive", "Encoders (live)", "انکوڈرز (لائیو)", "एन्कोडर (लाइव)"),
 ("Endpoints", "Endpoints", "آخری حدیں", "अंतिम सीमाएँ"),
 ("EndpointsComeFromCalibrationAdjust", "Endpoints come from calibration — adjust them in Setup.", "آخری حدیں کیلیبریشن سے آتی ہیں — انہیں Setup میں درست کریں۔", "अंतिम सीमाएँ कैलिब्रेशन से आती हैं — उन्हें Setup में समायोजित करें।"),
 ("Engine", "Engine", "انجن", "इंजन"),
 ("Engine200HzPipeline", "Engine (200 Hz pipeline)", "انجن (200 Hz پائپ لائن)", "इंजन (200 Hz पाइपलाइन)"),
 ("EveryVJoyButton132", "Every vJoy button 1–32 lights up while it is pressed. Outlined buttons are the assigned gear buttons.", "ہر vJoy بٹن 1–32 دبانے پر روشن ہوتا ہے۔ خاکہ والے بٹن تفویض کردہ گیئر بٹن ہیں۔", "हर vJoy बटन 1–32 दबाने पर जलता है। आउटलाइन वाले बटन असाइन किए गए गियर बटन हैं।"),
 ("ExitFullyFromTheTray", "Exit fully from the tray menu.", "ٹرے مینیو سے مکمل باہر نکلیں۔", "ट्रे मेनू से पूरी तरह बाहर निकलें।"),
 ("ForAMomentThenPress", "for a moment, then press each one", "ایک لمحے کے لیے، پھر ہر ایک کو دبائیں", "एक क्षण के लिए, फिर प्रत्येक को दबाएँ"),
 ("Fully", "fully", "مکمل طور پر", "पूरी तरह"),
 ("Function", "Function", "فنکشن", "फ़ंक्शन"),
 ("GameProcessName", "Game process name", "گیم پروسیس کا نام", "गेम प्रक्रिया का नाम"),
 ("GamesCannotSeeTheWheel", "Games cannot see the wheel until the vJoy driver is installed. Install BrunnerInnovation vJoy v2.2.2.0, then restart RSH.", "جب تک vJoy ڈرائیور انسٹال نہ ہو، گیمز وہیل کو نہیں دیکھ سکتیں۔ BrunnerInnovation vJoy v2.2.2.0 انسٹال کریں، پھر RSH دوبارہ شروع کریں۔", "जब तक vJoy ड्राइवर इंस्टॉल न हो, गेम व्हील को नहीं देख सकते। BrunnerInnovation vJoy v2.2.2.0 इंस्टॉल करें, फिर RSH पुनः आरंभ करें।"),
 ("GamesReadA", "Games read a", "گیمز پڑھتی ہیں ایک", "गेम पढ़ते हैं एक"),
 ("GateModeFollowsTheWheel", "Gate mode follows the wheel wiring (GET_CAPS). There is no app setting for Sequential / H-pattern yet.", "گیٹ موڈ وہیل کی وائرنگ (GET_CAPS) کے مطابق ہے۔ Sequential / H-pattern کے لیے ابھی کوئی ایپ سیٹنگ نہیں ہے۔", "गेट मोड व्हील की वायरिंग (GET_CAPS) के अनुसार है। Sequential / H-pattern के लिए अभी कोई ऐप सेटिंग नहीं है।"),
 ("GateTest", "Gate test", "گیٹ جانچ", "गेट जाँच"),
 ("GearAndPedalButtonsMapped", "Gear and pedal buttons, mapped to vJoy.", "گیئر اور پیڈل بٹن، vJoy سے منسلک۔", "गियर और पैडल बटन, vJoy से मैप किए गए।"),
 ("GearButtonsVJoy", "Gear buttons → vJoy", "گیئر بٹن → vJoy", "गियर बटन → vJoy"),
 ("GearDown", "Gear down", "گیئر نیچے", "गियर नीचे"),
 ("GearDownLabel", " · Gear down: ", " · گیئر نیچے: ", " · गियर नीचे: "),
 ("GearDownVJoyButton", " · Gear down → vJoy button ", " · گیئر نیچے → vJoy بٹن ", " · गियर नीचे → vJoy बटन "),
 ("GearDownVJoyButtonNumber", "Gear down vJoy button number", "گیئر نیچے vJoy بٹن نمبر", "गियर नीचे vJoy बटन संख्या"),
 ("GearDownVJoyButtonNumber2", "Gear down → vJoy button number", "گیئر نیچے → vJoy بٹن نمبر", "गियर नीचे → vJoy बटन संख्या"),
 ("GearUp", "Gear up", "گیئر اوپر", "गियर ऊपर"),
 ("GearUpAndDownShare", "Gear up and down share one button — use different numbers.", "گیئر اوپر اور نیچے ایک ہی بٹن استعمال کرتے ہیں — مختلف نمبر استعمال کریں۔", "गियर ऊपर और नीचे एक ही बटन साझा करते हैं — अलग संख्याएँ उपयोग करें।"),
 ("GearUpLabel", "Gear up: ", "گیئر اوپر: ", "गियर ऊपर: "),
 ("GearUpVJoy1Gear", "Gear up → vJoy 1 · Gear down → vJoy 2 · Buttons 1–32 live", "گیئر اوپر → vJoy 1 · گیئر نیچے → vJoy 2 · بٹن 1–32 لائیو", "गियर ऊपर → vJoy 1 · गियर नीचे → vJoy 2 · बटन 1–32 लाइव"),
 ("GearUpVJoyButton", "Gear up → vJoy button ", "گیئر اوپر → vJoy بٹن ", "गियर ऊपर → vJoy बटन "),
 ("GearUpVJoyButtonNumber", "Gear up vJoy button number", "گیئر اوپر vJoy بٹن نمبر", "गियर ऊपर vJoy बटन संख्या"),
 ("GearUpVJoyButtonNumber2", "Gear up → vJoy button number", "گیئر اوپر → vJoy بٹن نمبر", "गियर ऊपर → vJoy बटन संख्या"),
 ("GetVJoyDriver", "Get vJoy driver", "vJoy ڈرائیور لیں", "vJoy ड्राइवर प्राप्त करें"),
 ("HigherCalmerAtCentreLower", "Higher = calmer at centre. Lower = sharper. 0 = linear.", "زیادہ = مرکز پر پرسکون۔ کم = تیز۔ 0 = لکیری۔", "अधिक = केंद्र पर शांत। कम = तेज़। 0 = रैखिक।"),
 ("HigherSteadierSlightlySlower", "Higher = steadier, slightly slower.", "زیادہ = مستحکم، تھوڑا سست۔", "अधिक = स्थिर, थोड़ा धीमा।"),
 ("HoldTheWheelByThe", "• Hold the wheel by the rim.", "• وہیل کو رم سے پکڑیں۔", "• व्हील को रिम से पकड़ें।"),
 ("Horn", "Horn", "ہارن", "हॉर्न"),
 ("In", " in → ", " میں → ", " में → "),
 ("InVJoyConfEnableAxesX", "In vJoyConf, enable axes X, Y, Z and 2+ buttons.", "vJoyConf میں محور X، Y، Z اور 2+ بٹن فعال کریں۔", "vJoyConf में अक्ष X, Y, Z और 2+ बटन सक्षम करें।"),
 ("InYourGameBindThe", "In your game, bind the vJoy device. Finish — and drive.", "اپنی گیم میں vJoy ڈیوائس باندھیں۔ مکمل — اور چلائیں۔", "अपने गेम में vJoy डिवाइस बाँधें। समाप्त — और चलाएँ।"),
 ("InstallTheBrunnerInnovationVJoyV2", "Install the BrunnerInnovation vJoy v2.2.2.0 driver once.", "BrunnerInnovation vJoy v2.2.2.0 ڈرائیور ایک بار انسٹال کریں۔", "BrunnerInnovation vJoy v2.2.2.0 ड्राइवर एक बार इंस्टॉल करें।"),
 ("InvertAxis", "Invert axis", "محور الٹائیں", "अक्ष उलटें"),
 ("KeepBothPedals", "Keep both pedals", "دونوں پیڈل رکھیں", "दोनों पैडल रखें"),
 ("KeepPedalsReleasedBrieflyThen", ", keep pedals released briefly, then press each fully. Press again to stop.", "، پیڈلز کو تھوڑی دیر چھوڑا ہوا رکھیں، پھر ہر ایک کو مکمل دبائیں۔ روکنے کے لیے دوبارہ دبائیں۔", ", पैडल को थोड़ी देर छोड़ा हुआ रखें, फिर प्रत्येक को पूरी तरह दबाएँ। रोकने के लिए फिर से दबाएँ।"),
 ("LEDTestLEDTESTNeeds", "LED test (LED_TEST) needs a connected board with a session key. Pick a pattern and brightness below, then send the test — the strip runs it for 5 seconds.", "LED جانچ (LED_TEST) کے لیے سیشن کی کے ساتھ منسلک بورڈ چاہیے۔ نیچے پیٹرن اور چمک چنیں، پھر جانچ بھیجیں — اسٹرپ اسے 5 سیکنڈ چلائے گی۔", "LED जाँच (LED_TEST) के लिए सत्र कुंजी के साथ जुड़ा बोर्ड चाहिए। नीचे पैटर्न और चमक चुनें, फिर जाँच भेजें — स्ट्रिप उसे 5 सेकंड चलाएगी।"),
 ("Language", "Language", "زبان", "भाषा"),
 ("LanguageNote", "Applies immediately — no restart needed.", "فوراً لاگو ہوتی ہے — دوبارہ شروع کرنے کی ضرورت نہیں۔", "तुरंत लागू होती है — पुनः आरंभ की आवश्यकता नहीं।"),
 ("LaunchingAGameActivatesIts", "Launching a game activates its bound profile. One game at a time.", "گیم لانچ کرنے پر اس کی منسلک پروفائل فعال ہوتی ہے۔ ایک وقت میں ایک گیم۔", "गेम लॉन्च करने पर उसकी बाध्य प्रोफ़ाइल सक्रिय होती है। एक समय में एक गेम।"),
 ("LeaveTheWheel", "Leave the wheel", "وہیل کو چھوڑ دیں", "व्हील को छोड़ दें"),
 ("Link", "Link", "لنک", "लिंक"),
 ("Live", "Live", "لائیو", "लाइव"),
 ("LiveAccel", "Live: accel ", "لائیو: ایکسل ", "लाइव: एक्सेल "),
 ("LiveButtons", "Live buttons", "لائیو بٹن", "लाइव बटन"),
 ("LiveLink", "Live link", "لائیو لنک", "लाइव लिंक"),
 ("LiveOutputWhatGamesSee", "Live output — what games see", "لائیو آؤٹ پٹ — گیمز کیا دیکھتی ہیں", "लाइव आउटपुट — गेम क्या देखते हैं"),
 ("LiveReadingLabel", "Live reading: ", "لائیو ریڈنگ: ", "लाइव रीडिंग: "),
 ("LiveResponse", "Live response", "لائیو ردعمل", "लाइव प्रतिक्रिया"),
 ("LiveSteeringLabel", "Live steering: ", "لائیو اسٹیئرنگ: ", "लाइव स्टीयरिंग: "),
 ("LiveSteps", "Live steps", "لائیو اقدامات", "लाइव चरण"),
 ("LiveTest", "Live test", "لائیو جانچ", "लाइव जाँच"),
 ("LiveValues", "Live values", "لائیو قدریں", "लाइव मान"),
 ("LoadingPinTable", "Loading pin table…", "پن ٹیبل لوڈ ہو رہا ہے…", "पिन तालिका लोड हो रही है…"),
 ("LogLevel", "Log level", "لاگ کی سطح", "लॉग स्तर"),
 ("Logging", "Logging", "لاگنگ", "लॉगिंग"),
 ("Logs", "Logs", "لاگز", "लॉग"),
 ("LogsKeep7DaysIn", "Logs keep 7 days, in the config folder.", "لاگز 7 دن تک کنفیگ فولڈر میں رہتے ہیں۔", "लॉग 7 दिन तक कॉन्फ़िग फ़ोल्डर में रहते हैं।"),
 ("MaskLabel", " · Mask: ", " · ماسک: ", " · मास्क: "),
 ("Max", " · Max ", " · زیادہ سے زیادہ ", " · अधिकतम "),
 ("MaxLabel", "Max: ", "زیادہ سے زیادہ: ", "अधिकतम: "),
 ("Min", "Min ", "کم سے کم ", "न्यूनतम "),
 ("MinLabel", "Min: ", "کم سے کم: ", "न्यूनतम: "),
 ("ModuleButtons", "Wheel Buttons", "وہیل بٹن", "व्हील बटन"),
 ("ModuleClutch", "Clutch", "کلچ", "क्लच"),
 ("ModuleEncoders", "Encoders", "انکوڈرز", "एन्कोडर"),
 ("ModuleHandbrake", "Handbrake", "ہینڈ بریک", "हैंडब्रेक"),
 ("ModuleShifter", "Shifter", "شفٹر", "शिफ़्टर"),
 ("ModuleSubAccelerator", "Analog pedal · vJoy Axis Y", "اینالاگ پیڈل · vJoy Axis Y", "एनालॉग पैडल · vJoy Axis Y"),
 ("ModuleSubBrake", "Analog pedal · vJoy Axis Z", "اینالاگ پیڈل · vJoy Axis Z", "एनालॉग पैडल · vJoy Axis Z"),
 ("ModuleSubButtons", "Wheel buttons · vJoy 1–32", "وہیل بٹن · vJoy 1–32", "व्हील बटन · vJoy 1–32"),
 ("ModuleSubClutch", "Analog pedal · vJoy WClutch", "اینالاگ پیڈل · vJoy WClutch", "एनालॉग पैडल · vJoy WClutch"),
 ("ModuleSubEncoders", "Rotary encoders · cumulative steps", "روٹری انکوڈرز · مجموعی قدم", "रोटरी एन्कोडर · संचयी चरण"),
 ("ModuleSubHandbrake", "Analog lever · vJoy Slider", "اینالاگ لیور · vJoy Slider", "एनालॉग लीवर · vJoy Slider"),
 ("ModuleSubHorn", "Momentary button", "لمحاتی بٹن", "क्षणिक बटन"),
 ("ModuleSubLed", "Status LED strip", "اسٹیٹس LED اسٹرپ", "स्थिति LED स्ट्रिप"),
 ("ModuleSubShifter", "Digital gates · Sequential / H-pattern", "ڈیجیٹل گیٹس · Sequential / H-pattern", "डिजिटल गेट · Sequential / H-pattern"),
 ("ModuleSubSteering", "Wheel axis · vJoy Axis X", "وہیل محور · vJoy Axis X", "व्हील अक्ष · vJoy Axis X"),
 ("Ms", " ms", " ms", " ms"),
 ("NewProfileName", "New profile name", "نئی پروفائل کا نام", "नई प्रोफ़ाइल का नाम"),
 ("NextStep", "Next step", "اگلا مرحلہ", "अगला चरण"),
 ("NoDataFor250Ms", "no data for 250 ms. The safety watchdog is holding the controls centred.", "250 ms سے کوئی ڈیٹا نہیں۔ حفاظتی واچ ڈاگ کنٹرولز کو مرکز میں رکھے ہوئے ہے۔", "250 ms से कोई डेटा नहीं। सुरक्षा वॉचडॉग नियंत्रणों को केंद्र में रखे हुए है।"),
 ("NoDataWheelNotConnected", "No data — wheel not connected, live values are not updating.", "کوئی ڈیٹا نہیں — وہیل منسلک نہیں، لائیو قدریں اپ ڈیٹ نہیں ہو رہیں۔", "कोई डेटा नहीं — व्हील जुड़ा नहीं है, लाइव मान अपडेट नहीं हो रहे हैं।"),
 ("NoDataWheelNotConnected2", "No data — wheel not connected. ", "کوئی ڈیٹا نہیں — وہیل منسلک نہیں۔ ", "कोई डेटा नहीं — व्हील जुड़ा नहीं है। "),
 ("NoEncoderDataInThe", "No encoder data in the current stream — the v1 stream carries no encoders.", "موجودہ اسٹریم میں کوئی انکوڈر ڈیٹا نہیں — v1 اسٹریم میں انکوڈر نہیں ہوتے۔", "वर्तमान स्ट्रीम में कोई एन्कोडर डेटा नहीं है — v1 स्ट्रीम में एन्कोडर नहीं होते।"),
 ("NoFileChosen", "No file chosen", "کوئی فائل منتخب نہیں", "कोई फ़ाइल नहीं चुनी गई"),
 ("NoInternetWhileOnRSH", "No internet while on RSH — normal.", "RSH پر رہتے ہوئے انٹرنیٹ نہیں — عام بات ہے۔", "RSH पर रहते हुए इंटरनेट नहीं — सामान्य है।"),
 ("OTAPasswordLabel", "OTA password:", "OTA پاس ورڈ:", "OTA पासवर्ड:"),
 ("OnOffSettings", "On/off: Settings.", "آن/آف: Settings۔", "चालू/बंद: Settings।"),
 ("OneAtATimeAnd", ", one at a time, and hold it a second. The endpoints set themselves.", "، ایک وقت میں ایک، اور ایک سیکنڈ تھامے رکھیں۔ آخری حدیں خود سیٹ ہو جاتی ہیں۔", ", एक समय में एक, और एक सेकंड थामे रखें। अंतिम सीमाएँ स्वयं सेट हो जाती हैं।"),
 ("OneProfilePerGameSelecting", "One profile per game. Selecting activates it.", "ہر گیم کے لیے ایک پروفائل۔ منتخب کرنے پر فعال ہوتی ہے۔", "प्रति गेम एक प्रोफ़ाइल। चुनने पर यह सक्रिय होती है।"),
 ("OpenGameControllers", "Open Game Controllers", "گیم کنٹرولرز کھولیں", "गेम नियंत्रक खोलें"),
 ("OpenGameControllersJoyCpl", "Open Game Controllers (joy.cpl)", "گیم کنٹرولرز کھولیں (joy.cpl)", "गेम नियंत्रक खोलें (joy.cpl)"),
 ("OpenVJoyConf", "Open vJoyConf", "vJoyConf کھولیں", "vJoyConf खोलें"),
 ("OpenVJoyConfigure", "Open vJoy Configure", "vJoy Configure کھولیں", "vJoy Configure खोलें"),
 ("OptionalSendAFirmware", "Optional. Send a firmware", "اختیاری۔ فرم ویئر بھیجیں", "वैकल्पिक। फ़र्मवेयर भेजें"),
 ("Out", " out", " باہر", " बाहर"),
 ("OutputNeutralLabel", "Output neutral: ", "آؤٹ پٹ غیر جانبدار: ", "आउटपुट तटस्थ: "),
 ("PacketStats", "Packet stats", "پیکٹ اعداد", "पैकेट आँकड़े"),
 ("Password", "Password", "پاس ورڈ", "पासवर्ड"),
 ("PatternsTheBoardSupportsOFF", "Patterns the board supports: OFF · SOLID · BLINK · CHASE · RAINBOW.", "بورڈ جن پیٹرنز کو سپورٹ کرتا ہے: OFF · SOLID · BLINK · CHASE · RAINBOW۔", "बोर्ड जिन पैटर्न का समर्थन करता है: OFF · SOLID · BLINK · CHASE · RAINBOW।"),
 ("Pedals", "Pedals", "پیڈلز", "पैडल"),
 ("PerGameAutoSwitch", "Per-game auto-switch", "ہر گیم کے لیے خودکار تبدیلی", "प्रति गेम स्वचालित बदलें"),
 ("Performance", "Performance", "کارکردگی", "प्रदर्शन"),
 ("PickAModuleToTune", "Pick a module to tune it. Changes apply instantly in the game.", "ٹیون کرنے کے لیے ایک ماڈیول چنیں۔ تبدیلیاں گیم میں فوراً لاگو ہوتی ہیں۔", "ट्यून करने के लिए एक मॉड्यूल चुनें। बदलाव गेम में तुरंत लागू होते हैं।"),
 ("Pin", "Pin", "پن", "पिन"),
 ("PowerTheWheelJoinIts", "Power the wheel, join its Wi-Fi, and wait for packets.", "وہیل کو بجلی دیں، اس کی وائی فائی سے جڑیں، اور پیکٹس کا انتظار کریں۔", "व्हील को बिजली दें, उसके वाई-फ़ाई से जुड़ें, और पैकेट की प्रतीक्षा करें।"),
 ("PowerTheWheelOneClick", "Power the wheel. One click joins this PC to the", "وہیل کو بجلی دیں۔ ایک کلک سے یہ پی سی جڑ جاتا ہے", "व्हील को बिजली दें। एक क्लिक से यह PC जुड़ जाता है"),
 ("Press", "Press", "دبائیں", "दबाएँ"),
 ("PressEachButtonOnceOutput", "Press each button once. Output below is what games see.", "ہر بٹن ایک بار دبائیں۔ نیچے آؤٹ پٹ وہی ہے جو گیمز دیکھتی ہیں۔", "प्रत्येक बटन एक बार दबाएँ। नीचे आउटपुट वही है जो गेम देखते हैं।"),
 ("PressRamp", "Press ramp", "دبانے کا ریمپ", "दबाने का रैंप"),
 ("Pressed", " pressed", " دبایا گیا", " दबाया गया"),
 ("ProcessNameOnlyNoExe", "Process name only, no .exe — see Task Manager → Details.", "صرف پروسیس کا نام، .exe نہیں — Task Manager → Details دیکھیں۔", "केवल प्रक्रिया का नाम, .exe नहीं — Task Manager → Details देखें।"),
 ("ProcessedOutputWhatTheGame", "Processed output (what the game receives)", "پروسیس شدہ آؤٹ پٹ (گیم کو کیا ملتا ہے)", "संसाधित आउटपुट (गेम को क्या मिलता है)"),
 ("ProfileLabel", "Profile: ", "پروفائل: ", "प्रोफ़ाइल: "),
 ("PulsesPerStepIsAssignment", "Pulses per step is assignment — set it in Settings.", "فی قدم پلس تفویض ہے — اسے Settings میں سیٹ کریں۔", "प्रति चरण पल्स असाइनमेंट है — इसे Settings में सेट करें।"),
 ("R", "R", "R", "R"),
 ("RSH", "RSH", "RSH", "RSH"),
 ("RSHRealSimHub", "RSH — Real Sim Hub", "RSH — Real Sim Hub", "RSH — Real Sim Hub"),
 ("RSHSetsItselfUpLabel", "RSH sets itself up:", "RSH خود سیٹ اپ ہوتا ہے:", "RSH स्वयं सेटअप होता है:"),
 ("RSHSetup", "RSH Setup", "RSH Setup", "RSH Setup"),
 ("RangeFoundLabel", "Range found: ", "حد ملی: ", "सीमा मिली: "),
 ("RawLiveDataFromThe", "Raw live data from the wheel.", "وہیل سے خام لائیو ڈیٹا۔", "व्हील से कच्चा लाइव डेटा।"),
 ("RealSimHub", "Real Sim Hub", "Real Sim Hub", "Real Sim Hub"),
 ("ReleaseRamp", "Release ramp", "چھوڑنے کا ریمپ", "छोड़ने का रैंप"),
 ("Released", "released", "چھوڑا ہوا", "छोड़ा हुआ"),
 ("RemovesTinyShakeAtCentre", "Removes tiny shake at centre. Set the game's own deadzone to 0.", "مرکز پر ہلکی لرزش ختم کرتا ہے۔ گیم کا اپنا ڈیڈ زون 0 پر سیٹ کریں۔", "केंद्र पर हल्की कंपन हटाता है। गेम का अपना डेडज़ोन 0 पर सेट करें।"),
 ("ResponseCurve", "Response curve", "ردعمل کا خم", "प्रतिक्रिया वक्र"),
 ("RotationPresets", "Rotation presets", "گردش پری سیٹ", "घूर्णन प्रीसेट"),
 ("SaveCalibration", "Save calibration", "کیلیبریشن محفوظ کریں", "कैलिब्रेशन सहेजें"),
 ("SavedToTheActiveProfile", "Saved to the active profile.", "فعال پروفائل میں محفوظ ہوا۔", "सक्रिय प्रोफ़ाइल में सहेजा गया।"),
 ("Sequence", "Sequence", "ترتیب", "क्रम"),
 ("SetEachProfileSGame", "Set each profile's game on the Profiles page.", "ہر پروفائل کی گیم Profiles صفحے پر سیٹ کریں۔", "प्रत्येक प्रोफ़ाइल का गेम Profiles पृष्ठ पर सेट करें।"),
 ("SetYourWheelSReal", "Set your wheel's real steering and pedal ranges.", "اپنے وہیل کی اصل اسٹیئرنگ اور پیڈل حدود سیٹ کریں۔", "अपने व्हील की वास्तविक स्टीयरिंग और पैडल सीमाएँ सेट करें।"),
 ("ShiftDown", "Shift down", "شفٹ نیچے", "शिफ़्ट नीचे"),
 ("ShiftUp", "Shift up", "شفٹ اوپر", "शिफ़्ट ऊपर"),
 ("Smoothing", "Smoothing", "ہمواری", "स्मूदिंग"),
 ("StartMinimisedToTray", "Start minimised to tray", "ٹرے میں چھوٹا کر کے شروع کریں", "ट्रे में छोटा करके शुरू करें"),
 ("StartOver", "Start over", "دوبارہ شروع کریں", "फिर से शुरू करें"),
 ("StartPedalCapture", "Start pedal capture", "پیڈل کیپچر شروع کریں", "पैडल कैप्चर शुरू करें"),
 ("StartRangeCapture", "Start range capture", "حد کیپچر شروع کریں", "सीमा कैप्चर शुरू करें"),
 ("StartWithWindows", "Start with Windows", "ونڈوز کے ساتھ شروع کریں", "Windows के साथ शुरू करें"),
 ("StartupTray", "Startup & tray", "اسٹارٹ اپ اور ٹرے", "स्टार्टअप और ट्रे"),
 ("StatusLED", "Status LED", "اسٹیٹس LED", "स्थिति LED"),
 ("Steering", "Steering", "اسٹیئرنگ", "स्टीयरिंग"),
 ("SteeringLabel", "Steering: ", "اسٹیئرنگ: ", "स्टीयरिंग: "),
 ("SteeringRange", "Steering range", "اسٹیئرنگ کی حد", "स्टीयरिंग सीमा"),
 ("SteeringRaw", "Steering (raw)", "اسٹیئرنگ (خام)", "स्टीयरिंग (कच्चा)"),
 ("Step1Of7", "Step 1 of 7", "7 میں سے مرحلہ 1", "7 में से चरण 1"),
 ("StepsAdvanceOnTheirOwn", "Steps advance on their own when ready.", "مراحل تیار ہونے پر خود آگے بڑھتے ہیں۔", "चरण तैयार होने पर अपने आप आगे बढ़ते हैं।"),
 ("StopCapture", "Stop capture", "کیپچر روکیں", "कैप्चर रोकें"),
 ("SystemFollowsWindows", "System follows Windows.", "سسٹم ونڈوز کے مطابق ہے۔", "सिस्टम Windows के अनुसार है।"),
 ("TheBoardSPerDevice", "The board's per-device password (from pairing / first boot). Never a default.", "بورڈ کا فی ڈیوائس پاس ورڈ (پیئرنگ / پہلی بوٹ سے)۔ کبھی ڈیفالٹ نہیں۔", "बोर्ड का प्रति-डिवाइस पासवर्ड (पेयरिंग / पहली बूट से)। कभी डिफ़ॉल्ट नहीं।"),
 ("TheWheelIsNotSending", "The wheel is not sending this axis yet — tuning is saved and will apply when the wheel reports it.", "وہیل ابھی یہ محور نہیں بھیج رہا — ٹیوننگ محفوظ ہے اور جب وہیل بتائے گا تو لاگو ہوگی۔", "व्हील अभी यह अक्ष नहीं भेज रहा है — ट्यूनिंग सहेजी गई है और जब व्हील बताएगा तो लागू होगी।"),
 ("TheWheelSWiFi", "The wheel's Wi-Fi address is 192.168.4.1 (rsh.local). Only a private IPv4 address is accepted.", "وہیل کا وائی فائی پتہ 192.168.4.1 (rsh.local) ہے۔ صرف نجی IPv4 پتہ قبول کیا جاتا ہے۔", "व्हील का वाई-फ़ाई पता 192.168.4.1 (rsh.local) है। केवल निजी IPv4 पता स्वीकार किया जाता है।"),
 ("Theme", "Theme", "تھیم", "थीम"),
 ("ThisAppOnYourPC", "This app on your PC", "یہ ایپ آپ کے پی سی پر", "यह ऐप आपके PC पर"),
 ("ThisModuleIsOffTurn", "This module is off. Turn it on in Settings → Modules.", "یہ ماڈیول بند ہے۔ اسے Settings → Modules میں آن کریں۔", "यह मॉड्यूल बंद है। इसे Settings → Modules में चालू करें।"),
 ("Tips", "Tips", "مشورے", "सुझाव"),
 ("ToTheWheelNow", "to the wheel now.", "ابھی وہیل کو۔", "अभी व्हील को।"),
 ("TotalStepsLabel", "Total steps: ", "کل اقدامات: ", "कुल चरण: "),
 ("TurnFullyLeftAndRight", ", turn fully left and right, press again to stop.", "، مکمل بائیں اور دائیں گھمائیں، روکنے کے لیے دوبارہ دبائیں۔", ", पूरी तरह बाएँ और दाएँ घुमाएँ, रोकने के लिए फिर से दबाएँ।"),
 ("Type", "Type", "قسم", "प्रकार"),
 ("UploadToWheel", "Upload to wheel", "وہیل پر اپ لوڈ کریں", "व्हील पर अपलोड करें"),
 ("UseUniqueNumbersFrom1", "Use unique numbers from 1–32. Buttons 33–128 are not supported in this app — they are rejected, never remapped to 1–32.", "1–32 میں سے منفرد نمبر استعمال کریں۔ بٹن 33–128 اس ایپ میں سپورٹ نہیں ہیں — انہیں مسترد کر دیا جاتا ہے، کبھی 1–32 پر دوبارہ نہیں لگایا جاتا۔", "1–32 में से अद्वितीय संख्याएँ उपयोग करें। बटन 33–128 इस ऐप में समर्थित नहीं हैं — वे अस्वीकार कर दिए जाते हैं, कभी 1–32 पर पुनः मैप नहीं किए जाते।"),
 ("VJoy", "vJoy", "vJoy", "vJoy"),
 ("VJoyDevice", "vJoy device", "vJoy ڈیوائس", "vJoy डिवाइस"),
 ("VJoyDeviceNumber", "vJoy device number", "vJoy ڈیوائس نمبر", "vJoy डिवाइस संख्या"),
 ("VJoyDriverNotFound", "vJoy driver not found", "vJoy ڈرائیور نہیں ملا", "vJoy ड्राइवर नहीं मिला"),
 ("VJoyOutSteering", "vJoy out — steering ", "vJoy آؤٹ — اسٹیئرنگ ", "vJoy आउट — स्टीयरिंग "),
 ("VirtualJoystick", "virtual joystick.", "ورچوئل جوائے اسٹک۔", "वर्चुअल जॉयस्टिक।"),
 ("WaitingForPackets", "Waiting for packets…", "پیکٹس کا انتظار ہے…", "पैकेट की प्रतीक्षा है…"),
 ("WelcomeToRSH", "Welcome to RSH", "RSH میں خوش آمدید", "RSH में स्वागत है"),
 ("WhatTheBoardReportsPins", "What the board reports. Pins are read-only here — to change them, go to Settings → GPIO pins.", "بورڈ جو بتاتا ہے۔ یہاں پن صرف پڑھنے کے لیے ہیں — تبدیل کرنے کے لیے Settings → GPIO pins پر جائیں۔", "बोर्ड जो बताता है। यहाँ पिन केवल पढ़ने के लिए हैं — बदलने के लिए Settings → GPIO pins पर जाएँ।"),
 ("WheelAddress", "Wheel address", "وہیل کا پتہ", "व्हील पता"),
 ("WheelButtons", "Wheel buttons", "وہیل بٹن", "व्हील बटन"),
 ("WheelNotConnectedPowerThe", "Wheel not connected. Power the wheel and join the RSH WiFi network.", "وہیل منسلک نہیں۔ وہیل کو بجلی دیں اور RSH WiFi نیٹ ورک سے جڑیں۔", "व्हील जुड़ा नहीं है। व्हील को बिजली दें और RSH WiFi नेटवर्क से जुड़ें।"),
 ("WheelPassword", "Wheel password", "وہیل پاس ورڈ", "व्हील पासवर्ड"),
 ("WheelWiFi", "Wheel Wi-Fi", "وہیل وائی فائی", "व्हील वाई-फ़ाई"),
 ("WheelWiFiPassword", "Wheel Wi-Fi password", "وہیل وائی فائی پاس ورڈ", "व्हील वाई-फ़ाई पासवर्ड"),
 ("WhenAGameLaunchedFrom", "When a game launched from the Dashboard closes, its previous profile is restored automatically.", "جب Dashboard سے لانچ کی گئی گیم بند ہوتی ہے، تو اس کی پچھلی پروفائل خودکار بحال ہو جاتی ہے۔", "जब Dashboard से लॉन्च किया गया गेम बंद होता है, तो उसकी पिछली प्रोफ़ाइल स्वचालित रूप से बहाल हो जाती है।"),
 ("WiFi", "Wi-Fi.", "وائی فائی سے۔", "वाई-फ़ाई से।"),
 ("WiringPinTable", "Wiring — pin table", "وائرنگ — پن ٹیبل", "वायरिंग — पिन तालिका"),
 ("WrittenToTheSparePartition", "Written to the spare partition first — a failed upload changes nothing.", "پہلے اضافی پارٹیشن پر لکھا جاتا ہے — ناکام اپ لوڈ سے کچھ نہیں بدلتا۔", "पहले अतिरिक्त पार्टीशन पर लिखा जाता है — असफल अपलोड से कुछ नहीं बदलता।"),
 ("WrongDirectionLaterUseInvert", "• Wrong direction later? Use Invert steering in Tune.", "• بعد میں سمت غلط ہو؟ Tune میں Invert steering استعمال کریں۔", "• बाद में दिशा गलत हो? Tune में Invert steering उपयोग करें।"),
 ("YourProfiles", "Your profiles", "آپ کی پروفائلز", "आपकी प्रोफ़ाइल"),
]
assert len(STRINGS) == 337, len(STRINGS)
assert len({k for k, *_ in STRINGS}) == 337

from xml.sax.saxutils import escape

HEADER = """<?xml version="1.0" encoding="utf-8"?>
<root>
  <xsd:schema id="root" xmlns="" xmlns:xsd="http://www.w3.org/2001/XMLSchema" xmlns:msdata="urn:schemas-microsoft-com:xml-msdata">
    <xsd:import namespace="http://www.w3.org/XML/1998/namespace" />
    <xsd:element name="root" msdata:IsDataSet="true">
      <xsd:complexType>
        <xsd:choice maxOccurs="unbounded">
          <xsd:element name="metadata">
            <xsd:complexType>
              <xsd:sequence>
                <xsd:element name="value" type="xsd:string" minOccurs="0" />
              </xsd:sequence>
              <xsd:attribute name="name" use="required" type="xsd:string" />
              <xsd:attribute name="type" type="xsd:string" />
              <xsd:attribute name="mimetype" type="xsd:string" />
              <xsd:attribute ref="xml:space" />
            </xsd:complexType>
          </xsd:element>
          <xsd:element name="assembly">
            <xsd:complexType>
              <xsd:attribute name="alias" type="xsd:string" />
              <xsd:attribute name="name" type="xsd:string" />
            </xsd:complexType>
          </xsd:element>
          <xsd:element name="data">
            <xsd:complexType>
              <xsd:sequence>
                <xsd:element name="value" type="xsd:string" minOccurs="0" msdata:Ordinal="1" />
                <xsd:element name="comment" type="xsd:string" minOccurs="0" msdata:Ordinal="2" />
              </xsd:sequence>
              <xsd:attribute name="name" type="xsd:string" use="required" />
              <xsd:attribute name="type" type="xsd:string" />
              <xsd:attribute name="mimetype" type="xsd:string" />
              <xsd:attribute ref="xml:space" />
            </xsd:complexType>
          </xsd:element>
          <xsd:element name="resheader">
            <xsd:complexType>
              <xsd:sequence>
                <xsd:element name="value" type="xsd:string" minOccurs="0" msdata:Ordinal="1" />
              </xsd:sequence>
              <xsd:attribute name="name" type="xsd:string" use="required" />
            </xsd:complexType>
          </xsd:element>
        </xsd:choice>
      </xsd:complexType>
    </xsd:element>
  </xsd:schema>
  <resheader name="resmimetype">
    <value>text/microsoft-resx</value>
  </resheader>
  <resheader name="version">
    <value>2.0</value>
  </resheader>
  <resheader name="reader">
    <value>System.Resources.ResXResourceReader, System.Windows.Forms, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089</value>
  </resheader>
  <resheader name="writer">
    <value>System.Resources.ResXResourceWriter, System.Windows.Forms, Version=4.0.0.0, Culture=neutral, PublicKeyToken=b77a5c561934e089</value>
  </resheader>
"""

def main():
    base = os.path.dirname(os.path.abspath(__file__))
    for fname, idx in [("Strings.resx", 1), ("Strings.ur.resx", 2), ("Strings.hi.resx", 3)]:
        parts = [HEADER]
        for row in STRINGS:
            parts.append(f'  <data name="{row[0]}" xml:space="preserve">\n    <value>{escape(row[idx])}</value>\n  </data>\n')
        parts.append("</root>\n")
        with open(os.path.join(base, fname), "w", encoding="utf-8") as f:
            f.write("".join(parts))

    props = "\n".join(f'    public string {k} => Get(nameof({k}));' for k, *_ in STRINGS)
    template = """using System.ComponentModel;
using System.Globalization;
using System.Resources;
using System.Runtime.CompilerServices;

namespace RSH.App.Resources;

/// <summary>
/// Phase F i18n: user-facing strings - English (default) + Urdu + Hindi.
/// .resx files: Resources/Strings.resx (default/en), Strings.ur.resx,
/// Strings.hi.resx. XAML mein {Binding Key, Source={x:Static res:UiStrings.Instance}}
/// se istemal hota hai - x:Static ke bar-aks ye binding live hai: language
/// badalte hi UiStrings.Instance PropertyChanged(null) uthata hai aur tamam
/// bound strings dobara parhe jate hain, app restart ke baghair
/// (Settings - Appearance - Language). Culture null = OS UI culture.
/// Missing key par English (neutral resx) fallback - raw key naam kabhi
/// user ko nazar nahi aata (W8 note 2).
/// Regenerate: gen_strings.py (yahi table).
/// </summary>
public sealed class UiStrings : INotifyPropertyChanged
{
    public static UiStrings Instance { get; } = new UiStrings();

    private static readonly ResourceManager Manager = new("RSH.App.Resources.Strings", typeof(UiStrings).Assembly);
    private static readonly CultureInfo English = CultureInfo.GetCultureInfo("en");

    private UiStrings() { }

    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>Override karein to test / preview ke liye culture badli ja sake; null = CurrentUICulture (System).</summary>
    public static CultureInfo? Culture { get; private set; }

    /// <summary>
    /// Language lagao: "System" (null = OS UI culture), "en", "ur", "hi".
    /// Tamam bound UI strings foran badal jati hain (PropertyChanged(null));
    /// koi restart nahi. Ghalat value par "System" jaisa sulook hota hai.
    /// </summary>
    public static void ApplyLanguage(string? language)
    {
        Culture = language switch
        {
            "en" => CultureInfo.GetCultureInfo("en"),
            "ur" => CultureInfo.GetCultureInfo("ur"),
            "hi" => CultureInfo.GetCultureInfo("hi"),
            _ => null,
        };
        if (Culture is not null)
        {
            CultureInfo.CurrentUICulture = Culture;
            CultureInfo.DefaultThreadCurrentUICulture = Culture;
        }
        else
        {
            CultureInfo.DefaultThreadCurrentUICulture = null;
        }
        // null = tamam properties refresh hon (CallerMemberName se
        // sirf ApplyLanguage ka naam aata — bindings refresh na hoti).
        Instance.OnPropertyChanged(null);
    }

    /// <summary>Code-behind / ViewModel se ek string lo (wahi fallback jaisa XAML bindings).</summary>
    public static string GetString(string name) => Get(name);

    private static string Get(string name) =>
        // Pehle chuni hui culture, phir English (neutral resx), aakhir
        // mein key naam (sirf tab jab resx mein key sire se mojood hi na ho).
        Manager.GetString(name, Culture)
        ?? Manager.GetString(name, English)
        ?? name;

    private void OnPropertyChanged([CallerMemberName] string? propertyName = null) =>
        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));

{PROPS}
}
"""
    cs = template.replace("{PROPS}", props)
    with open(os.path.join(base, "UiStrings.cs"), "w", encoding="utf-8") as f:
        f.write(cs)
    print(f"resx OK: {len(STRINGS)} keys x 3 cultures + UiStrings.cs")

if __name__ == "__main__":
    main()
