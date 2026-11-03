package dev.linktolinux.companion

import android.content.Context
import android.content.res.ColorStateList
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.GradientDrawable
import android.graphics.drawable.RippleDrawable
import android.text.InputType
import android.view.Gravity
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.view.ViewCompat

class CompanionViews(private val context: Context) {
    val ink: Int = Color.rgb(32, 37, 43)
    val secondary: Int = Color.rgb(100, 112, 123)
    val accent: Int = Color.rgb(23, 110, 115)
    val line: Int = Color.rgb(221, 226, 230)
    val background: Int = Color.rgb(247, 248, 250)
    fun dp(value: Int): Int = (value * context.resources.displayMetrics.density).toInt()
    fun column(): LinearLayout = LinearLayout(context).apply { orientation = LinearLayout.VERTICAL }
    fun row(): LinearLayout = LinearLayout(context).apply { orientation = LinearLayout.HORIZONTAL; gravity = Gravity.CENTER_VERTICAL }
    fun surface(color: Int = Color.WHITE, radius: Int = 14, border: Boolean = true): GradientDrawable = GradientDrawable().apply {
        setColor(color)
        cornerRadius = dp(radius).toFloat()
        if (border) setStroke(dp(1), line)
    }
    fun text(content: String, size: Float = 14f, muted: Boolean = false, bold: Boolean = false): TextView = TextView(context).apply {
        text = content
        textSize = size
        setTextColor(if (muted) secondary else ink)
        setLineSpacing(dp(3).toFloat(), 1f)
        if (bold) setTypeface(typeface, Typeface.BOLD)
        layoutParams = LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(8) }
    }
    fun heading(content: String): TextView = text(content, 17f, bold = true).apply {
        ViewCompat.setAccessibilityHeading(this, true)
    }
    fun card(title: String): LinearLayout = column().apply {
        setPadding(dp(18), dp(18), dp(18), dp(18))
        background = surface()
        layoutParams = LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(14) }
        addView(heading(title))
    }
    fun field(hint: String, value: String, input: Int = InputType.TYPE_CLASS_TEXT): EditText = EditText(context).apply {
        this.hint = hint
        setText(value)
        inputType = input
        setSingleLine(true)
        textSize = 15f
        setTextColor(ink)
        setHintTextColor(secondary)
        setPadding(dp(12), dp(10), dp(12), dp(10))
        background = surface(radius = 8)
        minimumHeight = dp(48)
        layoutParams = LinearLayout.LayoutParams(-1, -2).apply { bottomMargin = dp(12) }
    }
    fun button(content: String, primary: Boolean = false, destructive: Boolean = false, action: () -> Unit): Button = Button(context).apply {
        text = content
        textSize = 14f
        isAllCaps = false
        letterSpacing = 0f
        minHeight = dp(48)
        minimumHeight = dp(48)
        minimumWidth = 0
        setPadding(dp(12), dp(8), dp(12), dp(8))
        setTextColor(ColorStateList(arrayOf(intArrayOf(-android.R.attr.state_enabled), intArrayOf()), intArrayOf(secondary, if (primary) Color.WHITE else if (destructive) Color.rgb(159, 48, 48) else ink)))
        background = RippleDrawable(ColorStateList.valueOf(Color.argb(35, 23, 110, 115)), surface(if (primary) accent else Color.WHITE, 8, !primary), null)
        stateListAnimator = null
        elevation = 0f
        setOnClickListener { action() }
        layoutParams = LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(8) }
    }
    fun pairedButtons(first: Button, second: Button): LinearLayout = row().apply {
        first.layoutParams = LinearLayout.LayoutParams(0, -2, 1f).apply { marginEnd = dp(5) }
        second.layoutParams = LinearLayout.LayoutParams(0, -2, 1f).apply { marginStart = dp(5) }
        addView(first); addView(second)
        layoutParams = LinearLayout.LayoutParams(-1, -2).apply { topMargin = dp(8) }
    }
}
