using System.Net.Sockets;
using System.Threading.Channels;

namespace Devolutions.IronVnc.Example;


class IronVnc
{

    private string host;

    private string username;
    private string password;
    private int port;
    private Session? session;

    private Framed<NetworkStream>? framed;

    public event EventHandler<FramebufferResized>? FramebufferResizedEvent;
    public event EventHandler<bool>? FrameBufferSetResizingEvent;
    public event EventHandler? ResolutionChangeEvent;
    public event EventHandler<ImgRegion>? FramebufferUpdatedEvent;
    public event EventHandler<Displays>? NewDisplaysAvailableEvent;

    public Channel<UserEvent> userEventChannel = Channel.CreateUnbounded<UserEvent>();

    public IronVnc(IronVncConfig config)
    {
        Log.InitWithEnv();
        this.username = config.Username;
        this.password = config.Password;
        this.host = config.Host;
        this.port = config.Port;

    }

    public async Task<ConnectResult> Connect()
    {
        var tcpStream = new TcpClient(host, port).GetStream();
        framed = new Framed<NetworkStream>(tcpStream);
        var authIdentity = GenericAuthIdentity.NewWithOsRand(username, password);
        var connector = ClientConnector.New(authIdentity, SecuritySelector.NewIronSecuritySelector());
        var writeBuf = WriteBuf.New();

        while (true)
        {
            writeBuf.Clear();
            nuint responseLen;
            var nextFrameInfo = connector.NextFrameInfo();
            if (nextFrameInfo != null)
            {
                var bytes = await framed.ReadNextFrame(nextFrameInfo);
                responseLen = connector.Step(bytes, writeBuf);
            }
            else
            {
                responseLen = connector.StepNoInput(writeBuf);
            }
            Console.WriteLine($"Response length: {responseLen}");
            var responseSlice = writeBuf.Filled();
            await framed.Write(responseSlice);

            var connectorState = connector.GetState();
            var stateType = connectorState.GetEnumType();
            if (stateType == ClientConnectorStateEnum.ExchangeSecurity)
            {
                Console.WriteLine("ExchangeSecurity");
            }
            else if (stateType == ClientConnectorStateEnum.TransportUpgrade)
            {
                Console.WriteLine("TransportUpgrade Needed");
                return ConnectResult.TransportUpgrade();
            }
            else if (stateType == ClientConnectorStateEnum.Connected)
            {
                Console.WriteLine("Connected");
                var connectedResult = connectorState.GetConnected();
                var result = ConnectResult.Connected(connectedResult);
                return result;
            }
            else
            {
                Console.WriteLine($"Unknown state {stateType}");
            }

        }
    }

    public CancellationTokenSource StartActiveSession(
        ReadyToProcess readyToProcess
    )
    {
        var outputBuffer = WriteBuf.New();
        var resourceLock = new object();
        var cancelTokenSource = new CancellationTokenSource();

        var postProcess = (MustPoll mustPoll) =>
        {
            var result = pollSessionEvent(mustPoll, outputBuffer);

            if (result.terminated)
            {
                cancelTokenSource.Cancel();
            }
            else
            {
                readyToProcess = result.readyToProcess!;
            }

            framed!.Write(outputBuffer.Filled()).Wait();
            outputBuffer.Clear();
        };

        var serverTask = async () =>
        {
            var frame = await framed!.ReadFrame((src) =>
            {
                lock (resourceLock)
                {
                    return session!.FindFrameLength(src, readyToProcess);
                }
            });

            if (frame != null)
            {
                lock (resourceLock)
                {
                    var mustPoll = session!.ProcessFrame(frame!, outputBuffer, readyToProcess);
                    postProcess(mustPoll);
                }
            }
        };

        var userTask = async () =>
        {
            var userEvent = await userEventChannel.Reader.ReadAsync();
            lock (resourceLock)
            {
                MustPoll? mustPoll = null;
                Action? action = null;
                if (userEvent.actionType == UserActionType.PointerMoved)
                {
                    var (x, y) = userEvent.PointerMovedData!.Value;
                    action = Action.PointerMoved((ushort)x, (ushort)y);
                }
                else if (userEvent.actionType == UserActionType.MouseButton)
                {
                    var (isDown, button) = userEvent.MouseButtonData!.Value;
                    action = Action.MouseButton(isDown, button);
                }
                else if (userEvent.actionType == UserActionType.Key)
                {
                    var (isDown, key) = userEvent.KeyData!.Value;
                    Console.WriteLine($"Received Key {key}");
                    action = Action.Key(isDown, key);
                }
                else if (userEvent.actionType == UserActionType.SetDesktopSize)
                {
                    var (width, height) = userEvent.SetDesktopSizeData!.Value;
                    action = Action.SetDesktopSize((ushort)width, (ushort)height, 0);
                }
                else
                {
                    throw new NotImplementedException();
                }
                mustPoll = session!.ProcessAction(action, outputBuffer, readyToProcess);
                postProcess(mustPoll);
            }
        };


        _ = Task.Run(async () =>
        {
            while (!cancelTokenSource.IsCancellationRequested)
            {
                await serverTask();
            }
        },
        cancelTokenSource.Token
        );

        _ = Task.Run(async () =>
        {
            while (!cancelTokenSource.IsCancellationRequested)
            {
                await userTask();
            }
        },
        cancelTokenSource.Token
        );

        return cancelTokenSource;
    }


    public async Task<PollSessionEventResult> IgniteSession(
        ConnectResult connectResult,
        ushort width,
        ushort height
    )
    {
        Console.WriteLine("Ignite session");
        var serverInit = connectResult.connected.GetServerInit();
        var builder = SessionBuilder.New(serverInit);
        var protocolVersion = connectResult.connected.GetVersion()!;
        var authenticatedData = connectResult.connected.GetAuthenticatedData();

        builder.WithDefaultEncoding();
        builder.WithPreset(Preset.NewRfc6143());
        builder.WithPreset(Preset.NewRfbext());
        builder.WithDefaultEncodingFilter(protocolVersion);
        builder.WithDesktopSize(width, height);

        if (protocolVersion.GetVersionConst() == ProtocolVersionConst.VArd)
        {
            if (authenticatedData == null)
            {
                throw new Exception("authenticated to ARD server, but session key is not set");
            }
            var ardPreset = Preset.NewArd(authenticatedData);
            builder.WithPreset(ardPreset);
            builder.WithSetupAction(SetupAction.NewSetViewerInfoAction());
            builder.WithSetupAction(SetupAction.NewArdAutoPasteboardActionSync());
        }

        var buildResult = builder.Build();
        var mustPoll = buildResult.GetMustPoll();
        session = buildResult.GetSession();

        var writeBuf = WriteBuf.New();

        var result = pollSessionEvent(mustPoll, writeBuf);

        await framed!.Write(writeBuf.Filled());
        Console.WriteLine("Ignite session done");
        return result;
    }

    private PollSessionEventResult pollSessionEvent(
        MustPoll mustPoll,
        WriteBuf writeBuf
        )
    {
        while (true)
        {
            var sessionEvent = session!.PopEvent(mustPoll);
            if (sessionEvent == null)
            {
                break;
            }
            var type = sessionEvent.GetEnumType();

            if (type == SessionEventEnum.FramebufferUpdated)
            {
                var frameBufferUpdated = sessionEvent.GetFramebufferUpdated();
                var frameBuffer = session.GetFrameBuffer();

                if (frameBufferUpdated.GetWidth() == 0)
                {
                    continue;
                }

                var img = frameBuffer.ToOwnedSubImage(
                    frameBufferUpdated.GetLeft(),
                    frameBufferUpdated.GetTop(),
                    frameBufferUpdated.GetWidth(),
                    frameBufferUpdated.GetHeight()
                );

                var imgRegion = ImgRegion.New(
                    frameBufferUpdated.GetLeft(),
                    frameBufferUpdated.GetTop(),
                    img
                );

                FramebufferUpdatedEvent?.Invoke(this, imgRegion);
            }
            else if (type == SessionEventEnum.FramebufferSetResizing)
            {
                var setResizing = sessionEvent.GetFramebufferSetResizing();
                FrameBufferSetResizingEvent?.Invoke(this, setResizing);
            }
            else if (type == SessionEventEnum.FramebufferResized)
            {
                var setResized = sessionEvent.GetFramebufferResized();
                FramebufferResizedEvent?.Invoke(this, setResized);
            }
            else if (type == SessionEventEnum.Callback)
            {
                var callback = sessionEvent.GetCallback();
                callback.Call(session, writeBuf);
            }
            else if (type == SessionEventEnum.Terminate)
            {
                return new PollSessionEventResult
                {
                    terminated = true
                };
            }
            else if (type == SessionEventEnum.NewDisplaysAvailable)
            {
                var displays = session.GetDisplays();
                NewDisplaysAvailableEvent?.Invoke(this, displays);
            }
            else
            {
                Console.WriteLine($"don't want to handle type {type}");
            }

        }


        var readyToProcess = session.PollEnd(mustPoll); ;
        return new PollSessionEventResult
        {
            readyToProcess = readyToProcess
        };
    }

    public class PollSessionEventResult
    {
        public bool terminated = false;
        public ReadyToProcess? readyToProcess;
    }
}

public enum ConnectResultType
{
    Connected,
    TransportUpgrade,
    Failed
}



public class ConnectResult
{
    public ConnectResultType connectionResultType { get; private set; }
    public ClientConnectorStateConnected? connected { get; private set; }

    public static ConnectResult Connected(ClientConnectorStateConnected connected)
    {
        return new ConnectResult()
        {
            connectionResultType = ConnectResultType.Connected,
            connected = connected
        };
    }

    public static ConnectResult TransportUpgrade()
    {
        return new ConnectResult()
        {
            connectionResultType = ConnectResultType.TransportUpgrade
        };
    }

    public static ConnectResult Failed()
    {
        return new ConnectResult()
        {
            connectionResultType = ConnectResultType.Failed
        };
    }
}

public class Ready
{

    public Ready(ReadyToProcess readyToProcess)
    {
        _process = readyToProcess;
    }
    private ReadyToProcess _process;

    public ReadyToProcess Process
    {
        set
        {
            _process = value;
            var callback = callbacks.Dequeue();
            if (callback != null)
            {
                callback(value);
            }
        }
    }

    private Queue<Func<ReadyToProcess, string>> callbacks = new();
    public void whenReady<T>(Func<ReadyToProcess, string> callback)
    {
        if (_process == null || _process.IsConsumed())
        {
            callbacks.Enqueue(callback);
        }

        callback(_process!);
    }
}
