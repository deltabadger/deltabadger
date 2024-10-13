
# this initializer automatically adds form validation errors from the model below the input field (inline)
# avoids having to add a class like
# <%= tag.p @subscription.errors[:eth_address].join(", ") if @subscription.errors[:eth_address].any? %>
# below each form field
# source: https://www.jorgemanrubia.com/2019/02/16/form-validations-with-html5-and-modern-rails/

ActionView::Base.field_error_proc = Proc.new do |html_tag, instance|
  fragment = Nokogiri::HTML.fragment(html_tag)
  field = fragment.at('input,select,textarea')

  model = instance.object
  error_message = model.errors.messages.values.join(', ')  # formatted as <error_message>
  # error_message = model.errors.full_messages.join(', ')  # formatted as <attribute_name> <error_message>

  html = if field
           field['class'] = "#{field['class']} is-invalid"
           html = <<-HTML
             #{fragment.to_s}
             <p class="db-form__info--invalid">#{error_message.upcase_first}</p>
           HTML
           html
         else
           html_tag
         end

  html.html_safe
end