import axios from 'axios';

const API_URL = '/api';

const element = document.getElementsByName('csrf-token')[0];
const token = element && element.getAttribute('content');

const client = axios.create({
  headers: { 'X-CSRF-Token': token },
});

const API = {
  get(endpoint, params) {
    const url = `${API_URL}/${endpoint}`;
    return client.request({ url, params, method: 'get' }).then(data => data.data);
  },

  post(endpoint, params) {
    return client.post(`/${endpoint}`, params).then(data => data.data);
  },

  getExchanges() {
    const url = `${API_URL}/exchanges`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  createApiKey(params) {
    const url = `${API_URL}/api_keys`;
    const ApiKeyParams = {
      exchange_id: params.exchangeId,
      key: params.key,
      secret: params.secret
    }
    return client.request({ url, data: { api_key: ApiKeyParams}, method: 'post' }).then(data => data.data);
  },

  createBot(params) {
    const url = `${API_URL}/bots`;
    const botParams= {
      exchange_id: params.exchangeId,
      type: params.type,
      price: params.price,
      currency: params.currency,
      interval: params.interval,
    }
    return client.request({ url, data: { bot: botParams}, method: 'post' }).then(data => data.data);
  },

  getBots() {
    const url = `${API_URL}/bots`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },

  startBot(botId) {
    const url = `${API_URL}/bots/${botId}/start`;
    return client.request({ url, params: {}, method: 'post' }).then(data => data.data);
  },

  stopBot(botId) {
    const url = `${API_URL}/bots/${botId}/stop`;
    return client.request({ url, params: {}, method: 'post' }).then(data => data.data);
  },

  removeBot(botId) {
    const url = `${API_URL}/bots/${botId}`;
    return client.request({ url, params: {}, method: 'delete' }).then(data => data.data);
  },

   getSubscription() {
    const url = `${API_URL}/subscriptions/check`;
    return client.request({ url, params: {}, method: 'get' }).then(data => data.data);
  },
};

export default API;
