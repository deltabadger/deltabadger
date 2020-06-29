class FormatReadableDuration < BaseService
  def call(duration)
    ActiveSupport::Duration.build(duration.to_i).inspect
  end
end
