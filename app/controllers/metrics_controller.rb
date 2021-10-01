require 'json'
class MetricsController < ApplicationController
  def index
    metrics_key = MetricsRepository.new.get_metrics_key
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    return render json: { data: JSON.parse(redis_client.get(metrics_key)) }.to_json unless redis_client.get(metrics_key).nil?

    MetricsRepository.new.update_metrics
    render json: { data: JSON.parse(redis_client.get(metrics_key)) }.to_json
  end
end
