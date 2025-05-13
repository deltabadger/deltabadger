module SettingsHelper
  def time_zone_with_offset(time_zone)
    tz = ActiveSupport::TimeZone[time_zone]
    offset = tz.now.utc_offset / 3600.0 # Convert seconds to hours
    offset_str = format_offset(offset) # Format as GMT +1:00
    "(GMT #{offset_str}) #{tz.name}"
  end

  def sorted_time_zone_options
    time_zones = ActiveSupport::TimeZone.all
    time_zones.map do |tz|
      [time_zone_with_offset(tz.name), tz.name, { data: { offset: tz.now.utc_offset } }]
    end.sort_by { |_, name, opts| [opts[:data][:offset], name] }
  end

  private

  def format_offset(offset)
    sign = offset >= 0 ? '+' : '-'
    hours = offset.abs.floor
    minutes = ((offset.abs % 1) * 60).to_i
    format('%<sign>s%<hours>d:%<minutes>02d', sign: sign, hours: hours, minutes: minutes)
  end
end
