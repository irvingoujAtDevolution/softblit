namespace Devolutions.IronVnc.Example
{
    public class UserEvent
    {

        public UserActionType actionType { get; private set; }
        public (double, double)? PointerMovedData { get; private set; }
        public (bool, MouseButton)? MouseButtonData { get; private set; }
        public (bool, Key)? KeyData { get; private set; }
        public (double, double)? SetDesktopSizeData { get; private set; }

        public static UserEvent PointerMoved(double x, double y)
        {
            return new UserEvent
            {
                actionType = UserActionType.PointerMoved,
                PointerMovedData = (x, y)
            };
        }

        public static UserEvent MouseButton(bool isDown, MouseButton button)
        {
            return new UserEvent
            {
                actionType = UserActionType.MouseButton,
                MouseButtonData = (isDown, button)
            };
        }

        public static UserEvent Key(bool isDown, Key key)
        {
            return new UserEvent
            {
                actionType = UserActionType.Key,
                KeyData = (isDown, key)
            };
        }

        public static UserEvent SetDesktopSize(double x, double y)
        {
            return new UserEvent
            {
                actionType = UserActionType.SetDesktopSize,
                SetDesktopSizeData = (x, y)
            };
        }
    }

    public enum UserActionType
    {
        PointerMoved = 0,
        MouseButton,
        Key,
        SetDesktopSize,
    }
}