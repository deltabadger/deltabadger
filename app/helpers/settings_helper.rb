module SettingsHelper
  # The sidebar: section, label, icon.
  SETTINGS_SECTIONS = [
    ['account', 'links.account', 'svg/24x24_account'],
    ['api_keys', 'settings.index.api_keys_section', 'svg/24x24_lock'],
    ['connect', 'links.connections', 'svg/24x24_sync'],
    ['mcp', 'settings.sections.mcp_server', 'svg/24x24_chat'],
    ['api', 'settings.rest.title', 'svg/24x24_doc']
  ].freeze

  # `?settings=<section>` on any page opens the dialog over it: how a language change comes back to
  # the page it was made on with the dialog still open. Only a real section; anything else, nothing.
  def settings_reopen_src
    section = params[:settings].to_s
    send("settings_#{section}_path") if SettingsController::SECTIONS.include?(section)
  end

  # The sidebar this user gets. Connect holds only container-wide sources (admin) and IBKR, so for
  # anyone else without IBKR it would open onto an empty pane.
  def settings_sections
    SETTINGS_SECTIONS.reject { |section, _, _| section == 'connect' && !settings_connect_visible? }
  end

  def settings_connect_visible?
    current_user.admin? || StockTradingSettings.ibkr_available? ||
      current_user.api_keys.joins(:exchange).exists?(exchanges: { type: 'Exchanges::Ibkr' }, key_type: 'trading')
  end

  # One row of the dialog: the label and its hint on the left, the control on the right. On a
  # narrow screen the control wraps under the text rather than squeezing it. `icon` is markup set
  # beside the label, level with its first line; the hint lines up under the label, not the icon.
  def settings_row(label, hint = nil, icon: nil, **options, &)
    tag.div(**options, class: ['srow', options[:class]]) do
      text = tag.div(class: ['srow__text', ('srow__text--icon' if icon)]) do
        safe_join([(tag.div(icon, class: 'srow__icon') if icon), tag.div(label, class: 'srow__label'),
                   (tag.div(hint, class: 'srow__hint') if hint.present?)])
      end
      safe_join([text, (tag.div(class: 'srow__control', &) if block_given?)])
    end
  end

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
