using System.ComponentModel;
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

    public string NavDashboard => Get(nameof(NavDashboard));
    public string NavTuning => Get(nameof(NavTuning));
    public string NavSetup => Get(nameof(NavSetup));
    public string NavButtons => Get(nameof(NavButtons));
    public string NavOutput => Get(nameof(NavOutput));
    public string NavProfiles => Get(nameof(NavProfiles));
    public string NavMonitor => Get(nameof(NavMonitor));
    public string NavSettings => Get(nameof(NavSettings));
    public string PageDashboard => Get(nameof(PageDashboard));
    public string PageTuning => Get(nameof(PageTuning));
    public string PageSetup => Get(nameof(PageSetup));
    public string PageButtons => Get(nameof(PageButtons));
    public string PageOutput => Get(nameof(PageOutput));
    public string PageProfiles => Get(nameof(PageProfiles));
    public string PageMonitor => Get(nameof(PageMonitor));
    public string PageSettings => Get(nameof(PageSettings));
    public string GameOutput => Get(nameof(GameOutput));
    public string RestartWheel => Get(nameof(RestartWheel));
    public string StartSetup => Get(nameof(StartSetup));
    public string OpenSetup => Get(nameof(OpenSetup));
    public string Dismiss => Get(nameof(Dismiss));
    public string RecalibrateNow => Get(nameof(RecalibrateNow));
    public string Launch => Get(nameof(Launch));
    public string Stop => Get(nameof(Stop));
    public string GameLauncher => Get(nameof(GameLauncher));
    public string ConnectWifi => Get(nameof(ConnectWifi));
    public string Retry => Get(nameof(Retry));
    public string RefreshPinTable => Get(nameof(RefreshPinTable));
    public string Back => Get(nameof(Back));
    public string Restart => Get(nameof(Restart));
    public string Linear => Get(nameof(Linear));
    public string Progressive => Get(nameof(Progressive));
    public string TestPattern => Get(nameof(TestPattern));
    public string LedPattern => Get(nameof(LedPattern));
    public string LedBrightness => Get(nameof(LedBrightness));
    public string Activate => Get(nameof(Activate));
    public string Duplicate => Get(nameof(Duplicate));
    public string Export => Get(nameof(Export));
    public string Import => Get(nameof(Import));
    public string Delete => Get(nameof(Delete));
    public string CreateProfile => Get(nameof(CreateProfile));
    public string AddGtPreset => Get(nameof(AddGtPreset));
    public string AddF1Preset => Get(nameof(AddF1Preset));
    public string ExportDiagnostics => Get(nameof(ExportDiagnostics));
    public string OpenLogsFolder => Get(nameof(OpenLogsFolder));
    public string Refresh => Get(nameof(Refresh));
    public string ApplyPinMap => Get(nameof(ApplyPinMap));
    public string Reset => Get(nameof(Reset));
    public string BackupNow => Get(nameof(BackupNow));
    public string Restore => Get(nameof(Restore));
    public string OpenConfigFolder => Get(nameof(OpenConfigFolder));
    public string ChangeWheelPassword => Get(nameof(ChangeWheelPassword));
    public string RunSetupWizard => Get(nameof(RunSetupWizard));
    public string Modules => Get(nameof(Modules));
    public string GpioPins => Get(nameof(GpioPins));
    public string Appearance => Get(nameof(Appearance));
    public string Data => Get(nameof(Data));
    public string Connected => Get(nameof(Connected));
    public string NotConnected => Get(nameof(NotConnected));
    public string Cancel => Get(nameof(Cancel));
    public string Close => Get(nameof(Close));
    public string Next => Get(nameof(Next));
    public string SkipWizard => Get(nameof(SkipWizard));
    public string CalibStep1CentreWheelPress => Get(nameof(CalibStep1CentreWheelPress));
    public string WizardStep1VJoyCheck => Get(nameof(WizardStep1VJoyCheck));
    public string CalibStep2Press => Get(nameof(CalibStep2Press));
    public string WizardStep2WifiConnect => Get(nameof(WizardStep2WifiConnect));
    public string WizardStep3WiringCheck => Get(nameof(WizardStep3WiringCheck));
    public string WizardStep4FirmwareUpdate => Get(nameof(WizardStep4FirmwareUpdate));
    public string Rotation540SuitsGtCars => Get(nameof(Rotation540SuitsGtCars));
    public string WizardStep5SteeringCalibration => Get(nameof(WizardStep5SteeringCalibration));
    public string WizardStep6PedalCalibration => Get(nameof(WizardStep6PedalCalibration));
    public string WizardStep7ButtonOutputTest => Get(nameof(WizardStep7ButtonOutputTest));
    public string AButtonHasNoTravel => Get(nameof(AButtonHasNoTravel));
    public string About => Get(nameof(About));
    public string Accel => Get(nameof(Accel));
    public string AccelBtnLabel => Get(nameof(AccelBtnLabel));
    public string AccelButton => Get(nameof(AccelButton));
    public string AccelLabel => Get(nameof(AccelLabel));
    public string Accelerator => Get(nameof(Accelerator));
    public string Accelerator2 => Get(nameof(Accelerator2));
    public string AcceleratorLabel => Get(nameof(AcceleratorLabel));
    public string AcceleratorRaw => Get(nameof(AcceleratorRaw));
    public string ActiveRulesGameProcessProfile => Get(nameof(ActiveRulesGameProcessProfile));
    public string AllVJoyButtonsPressTo => Get(nameof(AllVJoyButtonsPressTo));
    public string AnOffModuleNeverReaches => Get(nameof(AnOffModuleNeverReaches));
    public string AnalogPedalAndButtonCombine => Get(nameof(AnalogPedalAndButtonCombine));
    public string AndStillForTwoSeconds => Get(nameof(AndStillForTwoSeconds));
    public string Apply => Get(nameof(Apply));
    public string ApplyDevice => Get(nameof(ApplyDevice));
    public string AutoSwitchProfilePerGame => Get(nameof(AutoSwitchProfilePerGame));
    public string ResumeAutoSwitch => Get(nameof(ResumeAutoSwitch));
    public string UseCurrentCar => Get(nameof(UseCurrentCar));
    public string AxesAndButtons => Get(nameof(AxesAndButtons));
    public string Bin => Get(nameof(Bin));
    public string BrakeFrag => Get(nameof(BrakeFrag));
    public string Brake => Get(nameof(Brake));
    public string BrakeSpace => Get(nameof(BrakeSpace));
    public string BrakeBtnLabel => Get(nameof(BrakeBtnLabel));
    public string BrakeButton => Get(nameof(BrakeButton));
    public string BrakeLabel => Get(nameof(BrakeLabel));
    public string BrakeRaw => Get(nameof(BrakeRaw));
    public string ButtonsMaskLabel => Get(nameof(ButtonsMaskLabel));
    public string ButtonsPressed0 => Get(nameof(ButtonsPressed0));
    public string ButtonsPressedLabel => Get(nameof(ButtonsPressedLabel));
    public string CalibrateYourWheelAndPedals => Get(nameof(CalibrateYourWheelAndPedals));
    public string Calibration => Get(nameof(Calibration));
    public string CaptureCentre => Get(nameof(CaptureCentre));
    public string Centre => Get(nameof(Centre));
    public string CentreLabel => Get(nameof(CentreLabel));
    public string ChangeTheMappingOnThe => Get(nameof(ChangeTheMappingOnThe));
    public string ChangesSaveAutomaticallyToThis => Get(nameof(ChangesSaveAutomaticallyToThis));
    public string ChangesTheBoardSWi => Get(nameof(ChangesTheBoardSWi));
    public string CheckTheWiringTheBoard => Get(nameof(CheckTheWiringTheBoard));
    public string Checking => Get(nameof(Checking));
    public string CheckingVJoyConfiguration => Get(nameof(CheckingVJoyConfiguration));
    public string ChooseBin => Get(nameof(ChooseBin));
    public string ClearCalibration => Get(nameof(ClearCalibration));
    public string CloseButtonHidesToTray => Get(nameof(CloseButtonHidesToTray));
    public string ConfigFolderLabel => Get(nameof(ConfigFolderLabel));
    public string ConnectAutomatically => Get(nameof(ConnectAutomatically));
    public string ConnectThisPCToThe => Get(nameof(ConnectThisPCToThe));
    public string ConnectToRSH => Get(nameof(ConnectToRSH));
    public string ConnectToYourWheel => Get(nameof(ConnectToYourWheel));
    public string ConnectYourWheelCheckThe => Get(nameof(ConnectYourWheelCheckThe));
    public string Connection => Get(nameof(Connection));
    public string CopiesTheActiveProfileAt => Get(nameof(CopiesTheActiveProfileAt));
    public string Create => Get(nameof(Create));
    public string CumulativeEncoderCountsFromThe => Get(nameof(CumulativeEncoderCountsFromThe));
    public string CurrentGateLabel => Get(nameof(CurrentGateLabel));
    public string CurrentVJoyButtonMaskLabel => Get(nameof(CurrentVJoyButtonMaskLabel));
    public string DeadCentre => Get(nameof(DeadCentre));
    public string Deadzone => Get(nameof(Deadzone));
    public string Detail => Get(nameof(Detail));
    public string DeviceStatus => Get(nameof(DeviceStatus));
    public string DigitalPedalRamp => Get(nameof(DigitalPedalRamp));
    public string DonTForceItPast => Get(nameof(DonTForceItPast));
    public string EGAssettoCorsa => Get(nameof(EGAssettoCorsa));
    public string EGAssettoCorsaGT3 => Get(nameof(EGAssettoCorsaGT3));
    public string ESP32DevKitV1WiringAnalog => Get(nameof(ESP32DevKitV1WiringAnalog));
    public string Encoder1 => Get(nameof(Encoder1));
    public string Encoder1Label => Get(nameof(Encoder1Label));
    public string Encoder2 => Get(nameof(Encoder2));
    public string Encoder2Label => Get(nameof(Encoder2Label));
    public string Encoder3Label => Get(nameof(Encoder3Label));
    public string Encoder4Label => Get(nameof(Encoder4Label));
    public string EncodersLive => Get(nameof(EncodersLive));
    public string Endpoints => Get(nameof(Endpoints));
    public string EndpointsComeFromCalibrationAdjust => Get(nameof(EndpointsComeFromCalibrationAdjust));
    public string Engine => Get(nameof(Engine));
    public string Engine200HzPipeline => Get(nameof(Engine200HzPipeline));
    public string EveryVJoyButton132 => Get(nameof(EveryVJoyButton132));
    public string ExitFullyFromTheTray => Get(nameof(ExitFullyFromTheTray));
    public string ForAMomentThenPress => Get(nameof(ForAMomentThenPress));
    public string Fully => Get(nameof(Fully));
    public string Function => Get(nameof(Function));
    public string GameProcessName => Get(nameof(GameProcessName));
    public string GamesCannotSeeTheWheel => Get(nameof(GamesCannotSeeTheWheel));
    public string GamesReadA => Get(nameof(GamesReadA));
    public string GateModeFollowsTheWheel => Get(nameof(GateModeFollowsTheWheel));
    public string GateTest => Get(nameof(GateTest));
    public string GearAndPedalButtonsMapped => Get(nameof(GearAndPedalButtonsMapped));
    public string GearButtonsVJoy => Get(nameof(GearButtonsVJoy));
    public string GearDown => Get(nameof(GearDown));
    public string GearDownLabel => Get(nameof(GearDownLabel));
    public string GearDownVJoyButton => Get(nameof(GearDownVJoyButton));
    public string GearDownVJoyButtonNumber => Get(nameof(GearDownVJoyButtonNumber));
    public string GearDownVJoyButtonNumber2 => Get(nameof(GearDownVJoyButtonNumber2));
    public string GearUp => Get(nameof(GearUp));
    public string GearUpAndDownShare => Get(nameof(GearUpAndDownShare));
    public string GearUpLabel => Get(nameof(GearUpLabel));
    public string GearUpVJoy1Gear => Get(nameof(GearUpVJoy1Gear));
    public string GearUpVJoyButton => Get(nameof(GearUpVJoyButton));
    public string GearUpVJoyButtonNumber => Get(nameof(GearUpVJoyButtonNumber));
    public string GearUpVJoyButtonNumber2 => Get(nameof(GearUpVJoyButtonNumber2));
    public string GetVJoyDriver => Get(nameof(GetVJoyDriver));
    public string HigherCalmerAtCentreLower => Get(nameof(HigherCalmerAtCentreLower));
    public string HigherSteadierSlightlySlower => Get(nameof(HigherSteadierSlightlySlower));
    public string HoldTheWheelByThe => Get(nameof(HoldTheWheelByThe));
    public string Horn => Get(nameof(Horn));
    public string In => Get(nameof(In));
    public string InVJoyConfEnableAxesX => Get(nameof(InVJoyConfEnableAxesX));
    public string InYourGameBindThe => Get(nameof(InYourGameBindThe));
    public string InstallTheBrunnerInnovationVJoyV2 => Get(nameof(InstallTheBrunnerInnovationVJoyV2));
    public string InvertAxis => Get(nameof(InvertAxis));
    public string KeepBothPedals => Get(nameof(KeepBothPedals));
    public string KeepPedalsReleasedBrieflyThen => Get(nameof(KeepPedalsReleasedBrieflyThen));
    public string LEDTestLEDTESTNeeds => Get(nameof(LEDTestLEDTESTNeeds));
    public string Language => Get(nameof(Language));
    public string LanguageNote => Get(nameof(LanguageNote));
    public string LaunchingAGameActivatesIts => Get(nameof(LaunchingAGameActivatesIts));
    public string LeaveTheWheel => Get(nameof(LeaveTheWheel));
    public string Link => Get(nameof(Link));
    public string Live => Get(nameof(Live));
    public string LiveAccel => Get(nameof(LiveAccel));
    public string LiveButtons => Get(nameof(LiveButtons));
    public string LiveLink => Get(nameof(LiveLink));
    public string LiveOutputWhatGamesSee => Get(nameof(LiveOutputWhatGamesSee));
    public string LiveReadingLabel => Get(nameof(LiveReadingLabel));
    public string LiveResponse => Get(nameof(LiveResponse));
    public string LiveSteeringLabel => Get(nameof(LiveSteeringLabel));
    public string LiveSteps => Get(nameof(LiveSteps));
    public string LiveTest => Get(nameof(LiveTest));
    public string LiveValues => Get(nameof(LiveValues));
    public string LoadingPinTable => Get(nameof(LoadingPinTable));
    public string LogLevel => Get(nameof(LogLevel));
    public string Logging => Get(nameof(Logging));
    public string Logs => Get(nameof(Logs));
    public string LogsKeep7DaysIn => Get(nameof(LogsKeep7DaysIn));
    public string MaskLabel => Get(nameof(MaskLabel));
    public string Max => Get(nameof(Max));
    public string MaxLabel => Get(nameof(MaxLabel));
    public string Min => Get(nameof(Min));
    public string MinLabel => Get(nameof(MinLabel));
    public string ModuleButtons => Get(nameof(ModuleButtons));
    public string ModuleClutch => Get(nameof(ModuleClutch));
    public string ModuleEncoders => Get(nameof(ModuleEncoders));
    public string ModuleHandbrake => Get(nameof(ModuleHandbrake));
    public string ModuleShifter => Get(nameof(ModuleShifter));
    public string ModuleSubAccelerator => Get(nameof(ModuleSubAccelerator));
    public string ModuleSubBrake => Get(nameof(ModuleSubBrake));
    public string ModuleSubButtons => Get(nameof(ModuleSubButtons));
    public string ModuleSubClutch => Get(nameof(ModuleSubClutch));
    public string ModuleSubEncoders => Get(nameof(ModuleSubEncoders));
    public string ModuleSubHandbrake => Get(nameof(ModuleSubHandbrake));
    public string ModuleSubHorn => Get(nameof(ModuleSubHorn));
    public string ModuleSubLed => Get(nameof(ModuleSubLed));
    public string ModuleSubShifter => Get(nameof(ModuleSubShifter));
    public string ModuleSubSteering => Get(nameof(ModuleSubSteering));
    public string Ms => Get(nameof(Ms));
    public string NewProfileName => Get(nameof(NewProfileName));
    public string NextStep => Get(nameof(NextStep));
    public string NoDataFor250Ms => Get(nameof(NoDataFor250Ms));
    public string NoDataWheelNotConnected => Get(nameof(NoDataWheelNotConnected));
    public string NoDataWheelNotConnected2 => Get(nameof(NoDataWheelNotConnected2));
    public string NoEncoderDataInThe => Get(nameof(NoEncoderDataInThe));
    public string NoFileChosen => Get(nameof(NoFileChosen));
    public string NoInternetWhileOnRSH => Get(nameof(NoInternetWhileOnRSH));
    public string OTAPasswordLabel => Get(nameof(OTAPasswordLabel));
    public string OnOffSettings => Get(nameof(OnOffSettings));
    public string OneAtATimeAnd => Get(nameof(OneAtATimeAnd));
    public string OneProfilePerGameSelecting => Get(nameof(OneProfilePerGameSelecting));
    public string OpenGameControllers => Get(nameof(OpenGameControllers));
    public string OpenGameControllersJoyCpl => Get(nameof(OpenGameControllersJoyCpl));
    public string OpenVJoyConf => Get(nameof(OpenVJoyConf));
    public string OpenVJoyConfigure => Get(nameof(OpenVJoyConfigure));
    public string OptionalSendAFirmware => Get(nameof(OptionalSendAFirmware));
    public string Out => Get(nameof(Out));
    public string OutputNeutralLabel => Get(nameof(OutputNeutralLabel));
    public string PacketStats => Get(nameof(PacketStats));
    public string Password => Get(nameof(Password));
    public string PatternsTheBoardSupportsOFF => Get(nameof(PatternsTheBoardSupportsOFF));
    public string Pedals => Get(nameof(Pedals));
    public string PerGameAutoSwitch => Get(nameof(PerGameAutoSwitch));
    public string Performance => Get(nameof(Performance));
    public string PickAModuleToTune => Get(nameof(PickAModuleToTune));
    public string Pin => Get(nameof(Pin));
    public string PowerTheWheelJoinIts => Get(nameof(PowerTheWheelJoinIts));
    public string PowerTheWheelOneClick => Get(nameof(PowerTheWheelOneClick));
    public string Press => Get(nameof(Press));
    public string PressEachButtonOnceOutput => Get(nameof(PressEachButtonOnceOutput));
    public string PressRamp => Get(nameof(PressRamp));
    public string Pressed => Get(nameof(Pressed));
    public string ProcessNameOnlyNoExe => Get(nameof(ProcessNameOnlyNoExe));
    public string ProcessedOutputWhatTheGame => Get(nameof(ProcessedOutputWhatTheGame));
    public string ProfileLabel => Get(nameof(ProfileLabel));
    public string PulsesPerStepIsAssignment => Get(nameof(PulsesPerStepIsAssignment));
    public string R => Get(nameof(R));
    public string RSH => Get(nameof(RSH));
    public string RSHRealSimHub => Get(nameof(RSHRealSimHub));
    public string RSHSetsItselfUpLabel => Get(nameof(RSHSetsItselfUpLabel));
    public string RSHSetup => Get(nameof(RSHSetup));
    public string RangeFoundLabel => Get(nameof(RangeFoundLabel));
    public string RawLiveDataFromThe => Get(nameof(RawLiveDataFromThe));
    public string RealSimHub => Get(nameof(RealSimHub));
    public string ReleaseRamp => Get(nameof(ReleaseRamp));
    public string Released => Get(nameof(Released));
    public string RemovesTinyShakeAtCentre => Get(nameof(RemovesTinyShakeAtCentre));
    public string ResponseCurve => Get(nameof(ResponseCurve));
    public string RotationPresets => Get(nameof(RotationPresets));
    public string SaveCalibration => Get(nameof(SaveCalibration));
    public string SavedToTheActiveProfile => Get(nameof(SavedToTheActiveProfile));
    public string Sequence => Get(nameof(Sequence));
    public string SetEachProfileSGame => Get(nameof(SetEachProfileSGame));
    public string SetYourWheelSReal => Get(nameof(SetYourWheelSReal));
    public string ShiftDown => Get(nameof(ShiftDown));
    public string ShiftUp => Get(nameof(ShiftUp));
    public string Smoothing => Get(nameof(Smoothing));
    public string StartMinimisedToTray => Get(nameof(StartMinimisedToTray));
    public string StartOver => Get(nameof(StartOver));
    public string StartPedalCapture => Get(nameof(StartPedalCapture));
    public string StartRangeCapture => Get(nameof(StartRangeCapture));
    public string StartWithWindows => Get(nameof(StartWithWindows));
    public string StartupTray => Get(nameof(StartupTray));
    public string StatusLED => Get(nameof(StatusLED));
    public string Steering => Get(nameof(Steering));
    public string SteeringLabel => Get(nameof(SteeringLabel));
    public string SteeringRange => Get(nameof(SteeringRange));
    public string SteeringRaw => Get(nameof(SteeringRaw));
    public string Step1Of7 => Get(nameof(Step1Of7));
    public string StepsAdvanceOnTheirOwn => Get(nameof(StepsAdvanceOnTheirOwn));
    public string StopCapture => Get(nameof(StopCapture));
    public string SystemFollowsWindows => Get(nameof(SystemFollowsWindows));
    public string TheBoardSPerDevice => Get(nameof(TheBoardSPerDevice));
    public string TheWheelIsNotSending => Get(nameof(TheWheelIsNotSending));
    public string TheWheelSWiFi => Get(nameof(TheWheelSWiFi));
    public string Theme => Get(nameof(Theme));
    public string ThisAppOnYourPC => Get(nameof(ThisAppOnYourPC));
    public string ThisModuleIsOffTurn => Get(nameof(ThisModuleIsOffTurn));
    public string Tips => Get(nameof(Tips));
    public string ToTheWheelNow => Get(nameof(ToTheWheelNow));
    public string TotalStepsLabel => Get(nameof(TotalStepsLabel));
    public string TurnFullyLeftAndRight => Get(nameof(TurnFullyLeftAndRight));
    public string Type => Get(nameof(Type));
    public string UploadToWheel => Get(nameof(UploadToWheel));
    public string UseUniqueNumbersFrom1 => Get(nameof(UseUniqueNumbersFrom1));
    public string VJoy => Get(nameof(VJoy));
    public string VJoyDevice => Get(nameof(VJoyDevice));
    public string VJoyDeviceNumber => Get(nameof(VJoyDeviceNumber));
    public string VJoyDriverNotFound => Get(nameof(VJoyDriverNotFound));
    public string VJoyOutSteering => Get(nameof(VJoyOutSteering));
    public string VirtualJoystick => Get(nameof(VirtualJoystick));
    public string WaitingForPackets => Get(nameof(WaitingForPackets));
    public string WelcomeToRSH => Get(nameof(WelcomeToRSH));
    public string WhatTheBoardReportsPins => Get(nameof(WhatTheBoardReportsPins));
    public string WheelAddress => Get(nameof(WheelAddress));
    public string WheelButtons => Get(nameof(WheelButtons));
    public string WheelNotConnectedPowerThe => Get(nameof(WheelNotConnectedPowerThe));
    public string WheelPassword => Get(nameof(WheelPassword));
    public string WheelWiFi => Get(nameof(WheelWiFi));
    public string WheelWiFiPassword => Get(nameof(WheelWiFiPassword));
    public string WhenAGameLaunchedFrom => Get(nameof(WhenAGameLaunchedFrom));
    public string WiFi => Get(nameof(WiFi));
    public string WiringPinTable => Get(nameof(WiringPinTable));
    public string WrittenToTheSparePartition => Get(nameof(WrittenToTheSparePartition));
    public string WrongDirectionLaterUseInvert => Get(nameof(WrongDirectionLaterUseInvert));
    public string YourProfiles => Get(nameof(YourProfiles));
}
