
namespace Devolutions.IronVnc;

public class IronVncConfig
{
    public string Username { get; private set; } = String.Empty;
    public string Password { get; private set; } = String.Empty;
    public string Host { get; private set; }
    public int Port { get; private set; } = 5901;

    private IronVncConfig()
    {
        Host = string.Empty;
    }

    private static IronVncConfig? instance;
    public static IronVncConfig GetInstance()
    {
        if (instance == null)
        {
            instance = new IronVncConfig();
        }

        return instance;
    }


    public void ProcessArguments(string[] args)
    {
        if (args.Contains("--help") || args.Contains("-h"))
        {
            Console.WriteLine("Usage: Devolutions.IronVnc.Example [OPTIONS]");
            Console.WriteLine("Options:");
            Console.WriteLine("  --help, -h  Show this help message and exit");
            Console.WriteLine("  --username,-u  Username to use for authentication");
            Console.WriteLine("  --password,-p  Password to use for authentication");
            Console.WriteLine("  --host,-h  Host to connect to");
        }

        var username = Environment.GetEnvironmentVariable("IRONVNC_USERNAME");
        var password = Environment.GetEnvironmentVariable("IRONVNC_PASSWORD");
        var host = Environment.GetEnvironmentVariable("IRONVNC_HOST");
        var port = Environment.GetEnvironmentVariable("IRONVNC_PORT");
        port = port ?? "5900";
        for (int i = 0; i < args.Length; i++)
        {
            if (username == null && (args[i] == "--username" || args[i] == "-u"))
            {
                username = args[i + 1];
            }
            else if ((args[i] == "--password" || args[i] == "-p") && password == null)
            {
                password = args[i + 1];
            }
            else if ((args[i] == "--host" || args[i] == "-h") && host == null)
            {
                host = args[i + 1];
            }
        }

        if (string.IsNullOrEmpty(username) || string.IsNullOrEmpty(password) || string.IsNullOrEmpty(host))
        {
            Console.WriteLine("Missing required arguments");
            throw new ArgumentException("Missing required arguments");
        }

        Username = username;
        Password = password;
        Host = host;
        Port = int.Parse(port);
        Console.WriteLine($"Configuration set username: {username}, password: {password}, host: {host}");
    }
}


