require 'json'
class MetricsController < ApplicationController
  def index
    metrics = Metrics.new
    render json: { data: metrics.metrics_data }.to_json
  end
end
