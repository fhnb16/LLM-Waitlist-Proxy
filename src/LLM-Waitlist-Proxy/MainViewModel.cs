using GalaSoft.MvvmLight.Command;
using System;
using System.Collections.Concurrent;
using System.Collections.ObjectModel;
using System.ComponentModel;
using System.IO;
using System.Net;
using System.Net.Http;
using System.Text;
using System.Threading.Tasks;
using System.Windows;
using System.Windows.Input;
using Wpf.Ui.Input;
using Newtonsoft.Json;
using Newtonsoft.Json.Linq;
using System.Windows.Threading;
using System.Collections.Generic;
using System.Threading;

namespace LLM_Waitlist_Proxy
{

    public class MainViewModel : INotifyPropertyChanged
    {
        private HttpListener _listener;
        private Thread listenerThread;
        private readonly ConcurrentQueue<JsonRequest> _queue = new ConcurrentQueue<JsonRequest>();
        private JsonRequest _selectedRequest;
        private bool _isProcessingQueue;
        private bool _isServerRunning;

        private TelegramBotService telegramService;

        public ObservableCollection<JsonRequest> Requests { get; set; } = new ObservableCollection<JsonRequest>();
        public event PropertyChangedEventHandler PropertyChanged;

        public MainViewModel()
        {
            StartCommand = new RelayCommand(StartStopServer);
            ResetQueueCommand = new RelayCommand(ResetQueue);
        }

        // Свойство SelectedRequest и его содержимое для отображения в UI
        public JsonRequest SelectedRequest
        {
            get => _selectedRequest;
            set
            {
                _selectedRequest = value;
                OnPropertyChanged(nameof(SelectedRequest));
                OnPropertyChanged(nameof(SelectedRequestJsonContent));
            }
        }

        private string _consoleOutput;
        public string ConsoleOutput
        {
            get => _consoleOutput;
            set
            {
                _consoleOutput = value;
                OnPropertyChanged(nameof(ConsoleOutput)); // Уведомление об изменении
            }
        }

        public string SelectedRequestJsonContent => SelectedRequest != null ? SelectedRequest.ToString() : string.Empty;

        // Статус сервера и текст кнопки
        public bool IsServerRunning
        {
            get => _isServerRunning;
            set
            {
                _isServerRunning = value;
                OnPropertyChanged(nameof(IsServerRunning));
                OnPropertyChanged(nameof(ServerButtonText));
            }
        }

        public string Bearer { get; set; } = "no-key";

        public string ServerButtonText => IsServerRunning ? "Stop" : "Start";

        // Команды для кнопок
        public ICommand StartCommand { get; }
        public ICommand ResetQueueCommand { get; }

        protected virtual void OnPropertyChanged(string propertyName)
        {
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));
        }

        // Метод для обновления ConsoleOutput
        public void AppendConsoleOutput(string message)
        {
            ConsoleOutput = message + "\n" + ConsoleOutput;
        }

        //protected void OnPropertyChanged(string propertyName) => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(propertyName));

        // Запуск и остановка сервера
        private void StartStopServer()
        {
            if (IsServerRunning)
            {
                StopServer();
            }
            else
            {
                StartServer();
            }
        }

        private void StartServer()
        {
            AppendConsoleOutput("Starting server..");
            _listener = new HttpListener();
            _listener.Prefixes.Add("http://*:7070/v1/chat/completions/");
            _listener.Start();
            IsServerRunning = true;

            /*listenerThread = new Thread(new ThreadStart(async () =>
            {
                while (IsServerRunning)
                {
                    try
                    {
                        var context = await _listener.GetContextAsync();
                        var authorizationHeader = context.Request.Headers["Authorization"];
                        if (authorizationHeader != null && authorizationHeader.StartsWith("Bearer "))
                        {
                            string token = authorizationHeader.Substring(7).Trim();

                            this.Bearer = token;
                        }
                        AppendConsoleOutput("New request handled.");
                        ProcessRequest(context);
                    }catch(Exception ex)
                    {
                        AppendConsoleOutput("Listener thread exception: " + ex);
                    }
                }
            }));
            listenerThread.Start();*/

            Task.Run(async () =>
            {
                while (IsServerRunning)
                {
                    var context = await _listener.GetContextAsync();
                    var authorizationHeader = context.Request.Headers["Authorization"];
                    if (authorizationHeader != null && authorizationHeader.StartsWith("Bearer "))
                    {
                        string token = authorizationHeader.Substring(7).Trim();

                        this.Bearer = token;
                    }
                    AppendConsoleOutput("New request handled.");
                    ProcessRequest(context);
                }
            });
        }

        private void StopServer()
        {
            //listenerThread.Interrupt();
            _listener?.Stop();
            AppendConsoleOutput("Stopping server..");
            IsServerRunning = false;
        }

        private async void ProcessRequest(HttpListenerContext context)
        {
            using (var reader = new StreamReader(context.Request.InputStream, context.Request.ContentEncoding))
            {
                string json = await reader.ReadToEndAsync();
                var request = Newtonsoft.Json.JsonConvert.DeserializeObject<JsonRequest>(json);

                if (request != null)
                {
                    _queue.Enqueue(request);
                    Application.Current.Dispatcher.Invoke(() => Requests.Add(request));
                }
            }

            var response = Encoding.UTF8.GetBytes("{\"status\":\"Added to queue\"}");
            context.Response.StatusCode = 200;
            await context.Response.OutputStream.WriteAsync(response, 0, response.Length);
            context.Response.Close();
            AppendConsoleOutput("Added to queue.");

            ProcessQueue();
        }

        private async void ProcessQueue()
        {
            if (_isProcessingQueue) return;

            _isProcessingQueue = true;
            while (_queue.TryDequeue(out JsonRequest request))
            {
                AppendConsoleOutput("Request to LLM.");
                var responseJson = await SendRequest(request);
                AppendConsoleOutput("Request to Telegram.");
                await SendTelegramRequest(responseJson, request);
            }
            _isProcessingQueue = false;
        }

        // Ожидает ответа и возвращает его для последующей обработки

        private async Task<string> SendRequest(JsonRequest request)
        {
            using (var client = new HttpClient())
            {
                try
                {
                    //client.Timeout = TimeSpan.FromSeconds(1000);
                    client.DefaultRequestHeaders.Authorization =
                        new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer", this.Bearer);

                    var content = new StringContent(request.ToString(), Encoding.UTF8, "application/json");
                    var requestMessage = new HttpRequestMessage(HttpMethod.Post, "http://127.0.0.1:8080/v1/chat/completions")
                    {
                        Content = content
                    };

                    // Используем SendAsync с опцией ResponseHeadersRead для начала обработки данных в стриминге
                    using (var response = await client.SendAsync(requestMessage, HttpCompletionOption.ResponseHeadersRead))
                    {
                        response.EnsureSuccessStatusCode();

                        // Стрим для асинхронного чтения данных
                        using (var stream = await response.Content.ReadAsStreamAsync())
                        using (var reader = new StreamReader(stream))
                        {
                            StringBuilder buffer = new StringBuilder();
                            List<string> jsonObjects = new List<string>();
                            string line;
                            telegramService = new TelegramBotService();
                            await telegramService.SendStatusAsync(request.ChatID);

                            // Чтение данных построчно
                            while ((line = await reader.ReadLineAsync()) != null)
                            {
                                line = line.Trim();

                                if (line.StartsWith("data:"))
                                {
                                    line = line.Substring("data:".Length).Trim(); // Убираем "data:" и пробелы после него
                                }
                                // Проверяем, что строка содержит JSON и добавляем ее в список
                                if (!string.IsNullOrWhiteSpace(line))
                                {
                                    //jsonObjects.Add(line);
                                    //AppendConsoleOutput(line);
                                    JsonParser parser = new JsonParser();
                                    string ParsedLine = parser.GetContentFromJson(line);
                                    if (ParsedLine.Contains("#stopReading"))
                                    {
                                        AppendConsoleOutput("Finish reading stream..");
                                        break;
                                    }
                                    else
                                    {
                                        //AppendConsoleOutput(ParsedLine);
                                        buffer.Append(ParsedLine);
                                    }
                                }
                            }

                            // Объединяем накопленные JSON объекты в один массив JSON
                            //buffer.Append(string.Join(",", jsonObjects));

                            // Возвращаем полный JSON массив
                            string bufferString = buffer.ToString();
                            AppendConsoleOutput("Stream buffer: " + bufferString);
                            return bufferString;
                        }
                    }
                    /*using (var response = await client.SendAsync(requestMessage, HttpCompletionOption.ResponseHeadersRead))
                    {
                        response.EnsureSuccessStatusCode();

                        // Стрим для асинхронного чтения данных
                        using (var stream = await response.Content.ReadAsStreamAsync())
                        using (var reader = new StreamReader(stream))
                        {
                            StringBuilder buffer = new StringBuilder();
                            string line;

                            // Чтение данных построчно
                            while ((line = await reader.ReadLineAsync()) != null)
                            {
                                line = line.Substring(line.IndexOf('{') + 1);
                                if (line == "stopReading")
                                {
                                    AppendConsoleOutput("Finish reading stream..");
                                    break;
                                }
                                else
                                {
                                    AppendConsoleOutput(line);
                                    JsonParser parser = new JsonParser();
                                    string ParsedLine = parser.GetContentFromJson(line);
                                    AppendConsoleOutput(ParsedLine);
                                    buffer.Append(ParsedLine);
                                }
                                //MessageBox.Show(line);
                                // Добавляем поступающие данные в буфер
                            }

                            // Возвращаем полный ответ
                            AppendConsoleOutput("Stream buffer: " + buffer.ToString());
                            return buffer.ToString();
                        }
                    }*/
                }
                catch (Exception ex)
                {
                    AppendConsoleOutput("Can't send request to LLM, because: " + ex);
                    return null;
                }
            }
        }


        /*private async Task<string> SendRequest(JsonRequest request)
        {
            using (var client = new HttpClient())
            {
                try
                {
                    client.Timeout = TimeSpan.FromSeconds(1000);
                    client.DefaultRequestHeaders.Authorization = new System.Net.Http.Headers.AuthenticationHeaderValue("Bearer", this.Bearer);
                    var content = new StringContent(request.ToString(), Encoding.UTF8, "application/json");
                    var response = await client.PostAsync("http://localhost:8080/v1/chat/completions", content);
                    return await response.Content.ReadAsStringAsync();
                }
                catch (Exception ex)
                {
                    MessageBox.Show("Can't send request to LLM, because: "+ ex, "Error");
                    //StopServer();
                    return null;
                }
            }
        }*/

        // Отправка ответа в Telegram-бот или другую систему
        private async Task SendTelegramRequest(string jsonResponse, JsonRequest request)
        {
            if (jsonResponse != null)
            {
                telegramService = new TelegramBotService();

                /*dynamic llmResponseParsed = JObject.Parse(jsonResponse);
                var llmResponseText = llmResponseParsed.choices[0].message.content;*/

                string llmResponseText = jsonResponse/*parser.GetContentFromJson()*/;

                String escapedText = llmResponseText
    .Replace("*", "_")
    .Replace("</s>", "")
    .Replace("<|eot_id|>", "");

                string botResult = await telegramService.SendMessageAsync(
                    chatId: request.ChatID,
                    text: escapedText,
                    parseMode: "MarkdownV2",
                    replyToMessageId: request.ReplyTo
                );

                if (botResult != "Success")
                {
                    AppendConsoleOutput(botResult);
                }
                else
                {
                    _ = Application.Current.Dispatcher.BeginInvoke(
                      DispatcherPriority.Background,
                      new Action(() => Requests.RemoveAt(index: Requests.Count - 1)));
                }
            }
        }

        // Сброс очереди
        private void ResetQueue()
        {
            while (_queue.TryDequeue(out _)) { } // Очищаем очередь
            Application.Current.Dispatcher.Invoke(Requests.Clear);
        }
    }

    public class RelayCommand : ICommand
    {
        private readonly Action _execute;
        private readonly Func<bool> _canExecute;

        public RelayCommand(Action execute, Func<bool> canExecute = null)
        {
            _execute = execute;
            _canExecute = canExecute;
        }

        public bool CanExecute(object parameter) => _canExecute == null || _canExecute();

        public void Execute(object parameter) => _execute();

        public event EventHandler CanExecuteChanged
        {
            add => CommandManager.RequerySuggested += value;
            remove => CommandManager.RequerySuggested -= value;
        }
    }

public class JsonParser
    {
        public string GetContentFromJson(string jsonString)
        {
            try
            {
                JObject json = JObject.Parse(jsonString);

                string finish_reason = json["choices"]?[0]?["finish_reason"]?.ToString();

                if (finish_reason == "" || finish_reason.ToLower().Contains("none") || finish_reason.ToLower().Contains("null") || finish_reason == null)
                {
                    string content = json["choices"]?[0]?["delta"]?["content"]?.ToString();

                    return content;
                }
                else
                {
                    string content = json["choices"]?[0]?["delta"]?["content"]?.ToString();

                    if (content != null && content != "")
                    {
                        return content;
                    }
                    return "#stopReading@" + finish_reason;
                }
            }
            catch (Exception ex)
            {
                return "#stopReading@" + ex;
            }
        }
    }

}