require 'test_helper'

# field_error_proc marks the fragment it builds html_safe, so a model error message is rendered as
# markup. Now that one of those messages carries an exchange name and an asset symbol — both
# API-derived — it has to be escaped on the way in.
class InlineFormErrorsTest < ActiveSupport::TestCase
  test 'a model error message is escaped, not rendered as markup' do
    html = render_field_error('<script>alert(1)</script>')

    assert_includes html, '&lt;script&gt;'
    refute_includes html, '<script>'
  end

  test 'an ordinary message still renders, and the field is marked invalid' do
    html = render_field_error('must be greater than or equal to 5')

    assert_includes html, 'Must be greater than or equal to 5'
    assert_includes html, 'is-invalid'
  end

  private

  def render_field_error(message)
    instance = Struct.new(:error_message).new([message])
    ActionView::Base.field_error_proc.call('<input name="bot[amount]">', instance)
  end
end
