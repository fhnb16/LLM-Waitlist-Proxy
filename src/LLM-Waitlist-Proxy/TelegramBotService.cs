using System;
using System.Collections.Generic;
using System.Net.Http;
using System.Threading.Tasks;
using System.Windows;
using System.Net.Http.Json;
using System.Text;
using static System.Net.Mime.MediaTypeNames;

namespace LLM_Waitlist_Proxy
{

    public class TelegramBotService
    {
        private readonly string _botToken;
        private readonly HttpClient _httpClient;

        public TelegramBotService()
        {
            //_botToken = "6709758051:AAFuPj7IBXXR6gPOHNwMx0n8G66egfaUzlA";
            _httpClient = new HttpClient();
        }

        public async Task SendStatusAsync(string chatId)
        {
            // Формируем URL для метода sendMessage
            //string url = $"https://api.telegram.org/bot{_botToken}/sendChatAction";
            string url = $"https://bot.fhnb.ru/proxy" + "/sendChatAction";

            // Собираем параметры запроса
            var parameters = new FormUrlEncodedContent(new[]
            {
            new KeyValuePair<string, string>("chat_id", chatId),
            new KeyValuePair<string, string>("action", "typing")
            });

            var response = await _httpClient.PostAsync(url, parameters);

        }
        
        public async Task<string> SendMessageAsync(string chatId, string text, string parseMode = "MarkdownV2", string replyToMessageId = null)
        {
            // Формируем URL для метода sendMessage
            //string url = $"https://api.telegram.org/bot{_botToken}/sendMessage";
            string url = $"https://bot.fhnb.ru/proxy" + "/sendMessage";

            // Собираем параметры запроса
            var parameters = new FormUrlEncodedContent(new[]
            {
            new KeyValuePair<string, string>("chat_id", chatId),
            new KeyValuePair<string, string>("text", TelegramEscaper.EscapeMarkdownV2(text)),
            new KeyValuePair<string, string>("parse_mode", parseMode)
            });

            // Добавляем reply_to_message_id, если он указан
            if (replyToMessageId != null)
            {
                parameters = new FormUrlEncodedContent(new[]
                {
                new KeyValuePair<string, string>("chat_id", chatId),
                new KeyValuePair<string, string>("text", TelegramEscaper.EscapeMarkdownV2(text)),
                new KeyValuePair<string, string>("parse_mode", parseMode),
                new KeyValuePair<string, string>("reply_to_message_id", replyToMessageId)
            });
            }

            string UrlQuery = url + "?" + parameters.ReadAsStringAsync().Result;

            // Выполняем GET-запрос
            //var response = await _httpClient.GetAsync(UrlQuery);
            var response = await _httpClient.PostAsync(url, parameters);

            if (!response.IsSuccessStatusCode)
            {
                return "Cannot send request to telegram api: " + await response.Content.ReadAsStringAsync() + "\n\n" + UrlQuery;
            }
            else
            {
                return "Success";
            }
        }
    }

    public class TelegramEscaper
    {

        public static string EscapeMarkdownV2(string text)
        {
            // Список специальных символов для MarkdownV2
            char[] specialChars = { '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!' };

            var escapedText = new StringBuilder(text.Length);

            foreach (char c in text)
            {
                // Если символ находится в списке специальных, добавляем перед ним `\`
                if (Array.IndexOf(specialChars, c) >= 0)
                {
                    escapedText.Append('\\');
                }
                escapedText.Append(c);
            }

            return escapedText.ToString();
        }

        // Функция для экранирования строки
        public static string EscapeTelegramString(string input)
        {
            // Символы, которые нужно экранировать (включая символы с кодами от 1 до 126)
            string[] EscapeCharacters = new string[]
            {
        /*"_", */"*", "[", "]", "(", ")", "~", "`", ">", "#", "+", "-", "=", "|", "{", "}", ".", "!"
            };
            StringBuilder escapedString = new StringBuilder();

            foreach (char c in input)
            {
                // Если символ с кодом между 1 и 126, экранируем его
                if (c >= 1 && c <= 126)
                {
                    escapedString.Append('\\');
                    escapedString.Append(c);
                }
                // Если символ один из специальных, экранируем его
                else if (Array.Exists(EscapeCharacters, element => element == c.ToString()))
                {
                    escapedString.Append('\\');
                    escapedString.Append(c);
                }
                else
                {
                    escapedString.Append(c); // Оставляем символ без изменений
                }
            }

            return escapedString.ToString();
        }
    }
}