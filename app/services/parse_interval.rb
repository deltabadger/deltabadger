class ParseInterval < BaseService
  Error = StandardError

  INTERVALS = %w[month week day minutes].freeze

  def call(settings)
    interval = settings.fetch('interval')
    raise Error, 'Invalid interval' if !INTERVALS.include?(interval)

    case interval
    when 'minutes'
      1.minutes
    else
      1.public_send(interval)
    end
  end
end
