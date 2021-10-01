require 'json'
class MetricsController < ApplicationController
  def index
    redis_client = Redis.new(url: ENV.fetch('REDIS_AWS_URL'))
    return render json: { data: JSON.parse(redis_client.get('metrics')) }.to_json unless redis_client.get('metrics').nil?

    MetricsRepository.new.update_metrics
    render json: { data: JSON.parse(redis_client.get('metrics')) }.to_json
  end
end
