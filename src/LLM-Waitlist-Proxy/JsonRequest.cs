using Newtonsoft.Json;
using System;
using System.Collections.Generic;
using System.Linq;
using System.Text;
using System.Threading.Tasks;

namespace LLM_Waitlist_Proxy
{
    public class JsonRequest
    {
        [JsonProperty("messages")]
        public List<Message> Messages { get; set; }
        [JsonProperty("temperature")]
        public float Temperature { get; set; } = 0.50f;
        [JsonProperty("n_predict")]
        public int N_Predict { get; set; } = -1;
        [JsonProperty("max_tokens")]
        public int Max_tokens { get; set; } = 50;
        [JsonProperty("top_k")]
        public int Top_k { get; set; } = 30;
        [JsonProperty("top_p")]
        public float Top_p { get; set; } = 0.90f;
        [JsonProperty("mirostat")]
        public int Mirostat { get; set; } = 0;
        [JsonProperty("stream")]
        public bool Stream { get; set; } = false;
        [JsonProperty("chat_id")]
        public string ChatID { get; set; }
        [JsonProperty("reply_to")]
        public string ReplyTo { get; set; } = null;

        public string LastMessageContent
        {
            get
            {
                if (Messages != null && Messages.Count > 0)
                {
                    return Messages[Messages.Count - 1].Content;
                }
                return string.Empty;
            }
        }

        public JsonRequest()
        {
            Messages = new List<Message>();
        }

        public class Message
        {
            [JsonProperty("role")]
            public string Role { get; set; }

            /*[JsonProperty("name")]
            public string Name { get; set; }

            [JsonProperty("id")]
            public string ID { get; set; }

            [JsonProperty("tag")]
            public string Tag { get; set; }*/

            [JsonProperty("content")]
            public string Content { get; set; }
        }

        // Метод для сериализации объекта JsonRequest в JSON-формат
        public override string ToString()
        {
            return Newtonsoft.Json.JsonConvert.SerializeObject(this);
        }
    }
}
