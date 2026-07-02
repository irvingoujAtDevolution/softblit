using Avalonia.Input;

namespace Devolutions.IronVnc.Example
{
    internal class KeyMapper
    {
        private static readonly Dictionary<PhysicalKey, NamedKey> KeyToNamedKeyMap = new Dictionary<PhysicalKey, NamedKey>
{
    {PhysicalKey.Escape, NamedKey.Escape},
    {PhysicalKey.AltLeft, NamedKey.Alt},
    {PhysicalKey.AltRight, NamedKey.Alt},
    {PhysicalKey.Backspace, NamedKey.Backspace},
    {PhysicalKey.CapsLock, NamedKey.CapsLock},
    {PhysicalKey.ContextMenu, NamedKey.ContextMenu},
    {PhysicalKey.ControlLeft, NamedKey.Control},
    {PhysicalKey.ControlRight, NamedKey.Control},
    {PhysicalKey.Enter, NamedKey.Enter},
    {PhysicalKey.MetaLeft, NamedKey.Super},
    {PhysicalKey.MetaRight, NamedKey.Super},
    {PhysicalKey.ShiftLeft, NamedKey.Shift},
    {PhysicalKey.ShiftRight, NamedKey.Shift},
    {PhysicalKey.Space, NamedKey.Space},
    {PhysicalKey.Tab, NamedKey.Tab},
    {PhysicalKey.Convert, NamedKey.Convert},
    {PhysicalKey.KanaMode, NamedKey.KanaMode},
    {PhysicalKey.NonConvert, NamedKey.NonConvert},
    {PhysicalKey.Delete, NamedKey.Delete},
    {PhysicalKey.End, NamedKey.End},
    {PhysicalKey.Help, NamedKey.Help},
    {PhysicalKey.Home, NamedKey.Home},
    {PhysicalKey.Insert, NamedKey.Insert},
    {PhysicalKey.PageDown, NamedKey.PageDown},
    {PhysicalKey.PageUp, NamedKey.PageUp},
    {PhysicalKey.ArrowDown, NamedKey.ArrowDown},
    {PhysicalKey.ArrowLeft, NamedKey.ArrowLeft},
    {PhysicalKey.ArrowRight, NamedKey.ArrowRight},
    {PhysicalKey.ArrowUp, NamedKey.ArrowUp},
    {PhysicalKey.NumLock, NamedKey.NumLock},
    {PhysicalKey.F1, NamedKey.F1},
    {PhysicalKey.F2, NamedKey.F2},
    {PhysicalKey.F3, NamedKey.F3},
    {PhysicalKey.F4, NamedKey.F4},
    {PhysicalKey.F5, NamedKey.F5},
    {PhysicalKey.F6, NamedKey.F6},
    {PhysicalKey.F7, NamedKey.F7},
    {PhysicalKey.F8, NamedKey.F8},
    {PhysicalKey.F9, NamedKey.F9},
    {PhysicalKey.F10, NamedKey.F10},
    {PhysicalKey.F11, NamedKey.F11},
    {PhysicalKey.F12, NamedKey.F12},
    {PhysicalKey.F13, NamedKey.F13},
    {PhysicalKey.F14, NamedKey.F14},
    {PhysicalKey.F15, NamedKey.F15},
    {PhysicalKey.F16, NamedKey.F16},
    {PhysicalKey.F17, NamedKey.F17},
    {PhysicalKey.F18, NamedKey.F18},
    {PhysicalKey.F19, NamedKey.F19},
    {PhysicalKey.F20, NamedKey.F20},
    {PhysicalKey.F21, NamedKey.F21},
    {PhysicalKey.F22, NamedKey.F22},
    {PhysicalKey.F23, NamedKey.F23},
    {PhysicalKey.F24, NamedKey.F24},
    {PhysicalKey.PrintScreen, NamedKey.PrintScreen},
    {PhysicalKey.ScrollLock, NamedKey.ScrollLock},
    {PhysicalKey.Pause, NamedKey.Pause},
    {PhysicalKey.BrowserBack, NamedKey.BrowserBack},
    {PhysicalKey.BrowserFavorites, NamedKey.BrowserFavorites},
    {PhysicalKey.BrowserForward, NamedKey.BrowserForward},
    {PhysicalKey.BrowserHome, NamedKey.BrowserHome},
    {PhysicalKey.BrowserRefresh, NamedKey.BrowserRefresh},
    {PhysicalKey.BrowserSearch, NamedKey.BrowserSearch},
    {PhysicalKey.Eject, NamedKey.Eject},
    {PhysicalKey.LaunchApp1, NamedKey.LaunchApplication1},
    {PhysicalKey.LaunchApp2, NamedKey.LaunchApplication2},
    {PhysicalKey.LaunchMail, NamedKey.LaunchMail},
    {PhysicalKey.MediaPlayPause, NamedKey.MediaPlay},
    {PhysicalKey.MediaStop, NamedKey.MediaStop},
    {PhysicalKey.MediaTrackNext, NamedKey.MediaTrackNext},
    {PhysicalKey.MediaTrackPrevious, NamedKey.MediaTrackPrevious},
    {PhysicalKey.Power, NamedKey.Power},
    {PhysicalKey.Sleep, NamedKey.Standby},
    {PhysicalKey.AudioVolumeDown, NamedKey.AudioVolumeDown},
    {PhysicalKey.AudioVolumeMute, NamedKey.AudioVolumeMute},
    {PhysicalKey.AudioVolumeUp, NamedKey.AudioVolumeUp},
    {PhysicalKey.WakeUp, NamedKey.WakeUp},
    {PhysicalKey.Copy, NamedKey.Copy},
    {PhysicalKey.Cut, NamedKey.Cut},
    {PhysicalKey.Find, NamedKey.Find},
    {PhysicalKey.Open, NamedKey.Open},
    {PhysicalKey.Paste, NamedKey.Paste},
    {PhysicalKey.Select, NamedKey.Select},
    {PhysicalKey.Undo, NamedKey.Undo}
};

        private static readonly Dictionary<PhysicalKey, KeyLocation> KeyToLocationMap = new Dictionary<PhysicalKey, KeyLocation>
{
    {PhysicalKey.None, KeyLocation.Standard},
    {PhysicalKey.Backquote, KeyLocation.Standard},
    {PhysicalKey.Backslash, KeyLocation.Standard},
    {PhysicalKey.BracketLeft, KeyLocation.Standard},
    {PhysicalKey.BracketRight, KeyLocation.Standard},
    {PhysicalKey.Comma, KeyLocation.Standard},
    {PhysicalKey.Digit0, KeyLocation.Standard},
    {PhysicalKey.Digit1, KeyLocation.Standard},
    {PhysicalKey.Digit2, KeyLocation.Standard},
    {PhysicalKey.Digit3, KeyLocation.Standard},
    {PhysicalKey.Digit4, KeyLocation.Standard},
    {PhysicalKey.Digit5, KeyLocation.Standard},
    {PhysicalKey.Digit6, KeyLocation.Standard},
    {PhysicalKey.Digit7, KeyLocation.Standard},
    {PhysicalKey.Digit8, KeyLocation.Standard},
    {PhysicalKey.Digit9, KeyLocation.Standard},
    {PhysicalKey.Equal, KeyLocation.Standard},
    {PhysicalKey.IntlBackslash, KeyLocation.Standard},
    {PhysicalKey.IntlRo, KeyLocation.Standard},
    {PhysicalKey.IntlYen, KeyLocation.Standard},
    {PhysicalKey.A, KeyLocation.Standard},
    {PhysicalKey.B, KeyLocation.Standard},
    {PhysicalKey.C, KeyLocation.Standard},
    {PhysicalKey.D, KeyLocation.Standard},
    {PhysicalKey.E, KeyLocation.Standard},
    {PhysicalKey.F, KeyLocation.Standard},
    {PhysicalKey.G, KeyLocation.Standard},
    {PhysicalKey.H, KeyLocation.Standard},
    {PhysicalKey.I, KeyLocation.Standard},
    {PhysicalKey.J, KeyLocation.Standard},
    {PhysicalKey.K, KeyLocation.Standard},
    {PhysicalKey.L, KeyLocation.Standard},
    {PhysicalKey.M, KeyLocation.Standard},
    {PhysicalKey.N, KeyLocation.Standard},
    {PhysicalKey.O, KeyLocation.Standard},
    {PhysicalKey.P, KeyLocation.Standard},
    {PhysicalKey.Q, KeyLocation.Standard},
    {PhysicalKey.R, KeyLocation.Standard},
    {PhysicalKey.S, KeyLocation.Standard},
    {PhysicalKey.T, KeyLocation.Standard},
    {PhysicalKey.U, KeyLocation.Standard},
    {PhysicalKey.V, KeyLocation.Standard},
    {PhysicalKey.W, KeyLocation.Standard},
    {PhysicalKey.X, KeyLocation.Standard},
    {PhysicalKey.Y, KeyLocation.Standard},
    {PhysicalKey.Z, KeyLocation.Standard},
    {PhysicalKey.Minus, KeyLocation.Standard},
    {PhysicalKey.Period, KeyLocation.Standard},
    {PhysicalKey.Quote, KeyLocation.Standard},
    {PhysicalKey.Semicolon, KeyLocation.Standard},
    {PhysicalKey.Slash, KeyLocation.Standard},
    {PhysicalKey.AltLeft, KeyLocation.Left},
    {PhysicalKey.AltRight, KeyLocation.Right},
    {PhysicalKey.Backspace, KeyLocation.Standard},
    {PhysicalKey.CapsLock, KeyLocation.Standard},
    {PhysicalKey.ContextMenu, KeyLocation.Standard},
    {PhysicalKey.ControlLeft, KeyLocation.Left},
    {PhysicalKey.ControlRight, KeyLocation.Right},
    {PhysicalKey.Enter, KeyLocation.Standard},
    {PhysicalKey.MetaLeft, KeyLocation.Left},
    {PhysicalKey.MetaRight, KeyLocation.Right},
    {PhysicalKey.ShiftLeft, KeyLocation.Left},
    {PhysicalKey.ShiftRight, KeyLocation.Right},
    {PhysicalKey.Space, KeyLocation.Standard},
    {PhysicalKey.Tab, KeyLocation.Standard},
    {PhysicalKey.Convert, KeyLocation.Standard},
    {PhysicalKey.KanaMode, KeyLocation.Standard},
    {PhysicalKey.Lang1, KeyLocation.Standard},
    {PhysicalKey.Lang2, KeyLocation.Standard},
    {PhysicalKey.Lang3, KeyLocation.Standard},
    {PhysicalKey.Lang4, KeyLocation.Standard},
    {PhysicalKey.Lang5, KeyLocation.Standard},
    {PhysicalKey.NonConvert, KeyLocation.Standard},
    {PhysicalKey.Delete, KeyLocation.Standard},
    {PhysicalKey.End, KeyLocation.Standard},
    {PhysicalKey.Help, KeyLocation.Standard},
    {PhysicalKey.Home, KeyLocation.Standard},
    {PhysicalKey.Insert, KeyLocation.Standard},
    {PhysicalKey.PageDown, KeyLocation.Standard},
    {PhysicalKey.PageUp, KeyLocation.Standard},
    {PhysicalKey.ArrowDown, KeyLocation.Standard},
    {PhysicalKey.ArrowLeft, KeyLocation.Standard},
    {PhysicalKey.ArrowRight, KeyLocation.Standard},
    {PhysicalKey.ArrowUp, KeyLocation.Standard},
    {PhysicalKey.NumLock, KeyLocation.Numpad},
    {PhysicalKey.NumPad0, KeyLocation.Numpad},
    {PhysicalKey.NumPad1, KeyLocation.Numpad},
    {PhysicalKey.NumPad2, KeyLocation.Numpad},
    {PhysicalKey.NumPad3, KeyLocation.Numpad},
    {PhysicalKey.NumPad4, KeyLocation.Numpad},
    {PhysicalKey.NumPad5, KeyLocation.Numpad},
    {PhysicalKey.NumPad6, KeyLocation.Numpad},
    {PhysicalKey.NumPad7, KeyLocation.Numpad},
    {PhysicalKey.NumPad8, KeyLocation.Numpad},
    {PhysicalKey.NumPad9, KeyLocation.Numpad},
    {PhysicalKey.NumPadAdd, KeyLocation.Numpad},
    {PhysicalKey.NumPadClear, KeyLocation.Numpad},
    {PhysicalKey.NumPadComma, KeyLocation.Numpad},
    {PhysicalKey.NumPadDecimal, KeyLocation.Numpad},
    {PhysicalKey.NumPadDivide, KeyLocation.Numpad},
    {PhysicalKey.NumPadEnter, KeyLocation.Numpad},
    {PhysicalKey.NumPadEqual, KeyLocation.Numpad},
    {PhysicalKey.NumPadMultiply, KeyLocation.Numpad},
    {PhysicalKey.NumPadParenLeft, KeyLocation.Numpad},
    {PhysicalKey.NumPadParenRight, KeyLocation.Numpad},
    {PhysicalKey.NumPadSubtract, KeyLocation.Numpad},
    {PhysicalKey.Escape, KeyLocation.Standard},
    {PhysicalKey.F1, KeyLocation.Standard},
    {PhysicalKey.F2, KeyLocation.Standard},
    {PhysicalKey.F3, KeyLocation.Standard},
    {PhysicalKey.F4, KeyLocation.Standard},
    {PhysicalKey.F5, KeyLocation.Standard},
    {PhysicalKey.F6, KeyLocation.Standard},
    {PhysicalKey.F7, KeyLocation.Standard},
    {PhysicalKey.F8, KeyLocation.Standard},
    {PhysicalKey.F9, KeyLocation.Standard},
    {PhysicalKey.F10, KeyLocation.Standard},
    {PhysicalKey.F11, KeyLocation.Standard},
    {PhysicalKey.F12, KeyLocation.Standard},
    {PhysicalKey.F13, KeyLocation.Standard},
    {PhysicalKey.F14, KeyLocation.Standard},
    {PhysicalKey.F15, KeyLocation.Standard},
    {PhysicalKey.F16, KeyLocation.Standard},
    {PhysicalKey.F17, KeyLocation.Standard},
    {PhysicalKey.F18, KeyLocation.Standard},
    {PhysicalKey.F19, KeyLocation.Standard},
    {PhysicalKey.F20, KeyLocation.Standard},
    {PhysicalKey.F21, KeyLocation.Standard},
    {PhysicalKey.F22, KeyLocation.Standard},
    {PhysicalKey.F23, KeyLocation.Standard},
    {PhysicalKey.F24, KeyLocation.Standard},
    {PhysicalKey.PrintScreen, KeyLocation.Standard},
    {PhysicalKey.ScrollLock, KeyLocation.Standard},
    {PhysicalKey.Pause, KeyLocation.Standard},
    {PhysicalKey.BrowserBack, KeyLocation.Standard},
    {PhysicalKey.BrowserFavorites, KeyLocation.Standard},
    {PhysicalKey.BrowserForward, KeyLocation.Standard},
    {PhysicalKey.BrowserHome, KeyLocation.Standard},
    {PhysicalKey.BrowserRefresh, KeyLocation.Standard},
    {PhysicalKey.BrowserSearch, KeyLocation.Standard},
    {PhysicalKey.BrowserStop, KeyLocation.Standard},
    {PhysicalKey.Eject, KeyLocation.Standard},
    {PhysicalKey.LaunchApp1, KeyLocation.Standard},
    {PhysicalKey.LaunchApp2, KeyLocation.Standard},
    {PhysicalKey.LaunchMail, KeyLocation.Standard},
    {PhysicalKey.MediaPlayPause, KeyLocation.Standard},
    {PhysicalKey.MediaSelect, KeyLocation.Standard},
    {PhysicalKey.MediaStop, KeyLocation.Standard},
    {PhysicalKey.MediaTrackNext, KeyLocation.Standard},
    {PhysicalKey.MediaTrackPrevious, KeyLocation.Standard},
    {PhysicalKey.Power, KeyLocation.Standard},
    {PhysicalKey.Sleep, KeyLocation.Standard},
    {PhysicalKey.AudioVolumeDown, KeyLocation.Standard},
    {PhysicalKey.AudioVolumeMute, KeyLocation.Standard},
    {PhysicalKey.AudioVolumeUp, KeyLocation.Standard},
    {PhysicalKey.WakeUp, KeyLocation.Standard},
    {PhysicalKey.Again, KeyLocation.Standard},
    {PhysicalKey.Copy, KeyLocation.Standard},
    {PhysicalKey.Cut, KeyLocation.Standard},
    {PhysicalKey.Find, KeyLocation.Standard},
    {PhysicalKey.Open, KeyLocation.Standard},
    {PhysicalKey.Paste, KeyLocation.Standard},
    {PhysicalKey.Props, KeyLocation.Standard},
    {PhysicalKey.Select, KeyLocation.Standard},
    {PhysicalKey.Undo, KeyLocation.Standard}
};

        private static readonly Dictionary<PhysicalKey, char> KeyToCharMap = new Dictionary<PhysicalKey, char>
{
    {PhysicalKey.A, 'a'},
    {PhysicalKey.B, 'b'},
    {PhysicalKey.C, 'c'},
    {PhysicalKey.D, 'd'},
    {PhysicalKey.E, 'e'},
    {PhysicalKey.F, 'f'},
    {PhysicalKey.G, 'g'},
    {PhysicalKey.H, 'h'},
    {PhysicalKey.I, 'i'},
    {PhysicalKey.J, 'j'},
    {PhysicalKey.K, 'k'},
    {PhysicalKey.L, 'l'},
    {PhysicalKey.M, 'm'},
    {PhysicalKey.N, 'n'},
    {PhysicalKey.O, 'o'},
    {PhysicalKey.P, 'p'},
    {PhysicalKey.Q, 'q'},
    {PhysicalKey.R, 'r'},
    {PhysicalKey.S, 's'},
    {PhysicalKey.T, 't'},
    {PhysicalKey.U, 'u'},
    {PhysicalKey.V, 'v'},
    {PhysicalKey.W, 'w'},
    {PhysicalKey.X, 'x'},
    {PhysicalKey.Y, 'y'},
    {PhysicalKey.Z, 'z'},
    {PhysicalKey.Digit0, '0'},
    {PhysicalKey.Digit1, '1'},
    {PhysicalKey.Digit2, '2'},
    {PhysicalKey.Digit3, '3'},
    {PhysicalKey.Digit4, '4'},
    {PhysicalKey.Digit5, '5'},
    {PhysicalKey.Digit6, '6'},
    {PhysicalKey.Digit7, '7'},
    {PhysicalKey.Digit8, '8'},
    {PhysicalKey.Digit9, '9'},
    {PhysicalKey.Comma, ','},
    {PhysicalKey.Period, '.'},
    {PhysicalKey.Semicolon, ';'},
    {PhysicalKey.Quote, '\''},
    {PhysicalKey.BracketLeft, '['},
    {PhysicalKey.BracketRight, ']'},
    {PhysicalKey.Backslash, '\\'},
    {PhysicalKey.Slash, '/'},
    {PhysicalKey.Minus, '-'},
    {PhysicalKey.Equal, '='},
    {PhysicalKey.Backquote, '`'},
    {PhysicalKey.Space, ' '},
    {PhysicalKey.IntlBackslash, '\\'}, // UK Keyboard
    {PhysicalKey.IntlRo, '\\'}, // Japanese Keyboard
    {PhysicalKey.IntlYen, '¥'}, // Japanese Keyboard
    {PhysicalKey.NumPad0, '0'},
    {PhysicalKey.NumPad1, '1'},
    {PhysicalKey.NumPad2, '2'},
    {PhysicalKey.NumPad3, '3'},
    {PhysicalKey.NumPad4, '4'},
    {PhysicalKey.NumPad5, '5'},
    {PhysicalKey.NumPad6, '6'},
    {PhysicalKey.NumPad7, '7'},
    {PhysicalKey.NumPad8, '8'},
    {PhysicalKey.NumPad9, '9'},
    {PhysicalKey.NumPadAdd, '+'},
    {PhysicalKey.NumPadComma, ','},
    {PhysicalKey.NumPadDecimal, '.'},
    {PhysicalKey.NumPadDivide, '/'},
    {PhysicalKey.NumPadMultiply, '*'},
    {PhysicalKey.NumPadSubtract, '-'},
    {PhysicalKey.NumPadEqual, '='},
};


        public static Key[] map(PhysicalKey key, string? unicodeKey)
        {
            NamedKey? namedKey = ToNamedKey(key);
            KeyLocation? location = toKeyLocation(key);

            if (namedKey != null && location != null)
            {
                return [KeyCreator.FromNamedKey(namedKey.Value, location.Value)];
            }

            // When a key modifier exist, the unicode will be null, in such case we look up for keys in mapper
            if (unicodeKey == null && KeyToCharMap.ContainsKey(key))
            {
                return [KeyCreator.FromChar(KeyToCharMap[key])];
            }
            else if (unicodeKey == null)
            {
                throw new ArgumentException("Not a unicode char, not a named key, not a recoginized physical key");
            }

            Key[] keys = ToUnicodeChar(unicodeKey);

            return keys;
        }

        private static Key[] ToUnicodeChar(string unicodeKey)
        {
            char[] array = unicodeKey.ToArray();
            Key[] keys = new Key[array.Length];
            for (int i = 0; i < array.Length; i++)
            {
                char character = array[i];
                keys[i] = KeyCreator.FromChar(character);
            }
            return keys;
        }

        private static KeyLocation? toKeyLocation(PhysicalKey key)
        {
            if (KeyToLocationMap.ContainsKey(key))
            {
                return KeyToLocationMap[key];
            }
            return null;
        }

        private static NamedKey? ToNamedKey(PhysicalKey key)
        {
            if (KeyToNamedKeyMap.ContainsKey(key))
            {
                return KeyToNamedKeyMap[key];
            }
            return null;
        }
    }
}